//! The debugger in the code itself (the debugger overhaul's first phase,
//! `.scratch/debugger-overhaul.html`): breakpoints and the stop in the
//! gutter, the stopped line tinted, and the stopped frame's values written
//! at the ends of the lines above the stop -- in whichever pane shows the
//! file, never a workspace of its own. And DEBUG, the mode a stop puts
//! the editor in: one key steps, Esc goes back to Normal to read or edit.
//! The F-keys do the same in every mode.

use std::path::{Path, PathBuf};

use fenix_keymap::{KeyCode, KeyPress, Mods, NamedKey as FenixNamedKey};

use super::debug_breakpoints::BreakpointLook;
use super::*;

/// What a key does to the debug session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DebugAction {
    StartOrContinue,
    StepOver,
    StepIn,
    StepOut,
    RunToCursor,
    Pause,
    Stop,
    Restart,
    ToggleBreakpoint,
    /// Esc in DEBUG: back to Normal, stopped still.
    Leave,
}

/// DEBUG's own keys -- `n s o c r`, and Esc back to Normal. Every other
/// key is Normal's: `j`, `/`, `gd` read the code as ever.
pub(super) fn debug_mode_action(key: KeyPress) -> Option<DebugAction> {
    if key.mods != Mods::default() {
        return None;
    }
    Some(match key.code {
        KeyCode::Char('n') => DebugAction::StepOver,
        KeyCode::Char('s') => DebugAction::StepIn,
        KeyCode::Char('o') => DebugAction::StepOut,
        KeyCode::Char('c') => DebugAction::StartOrContinue,
        KeyCode::Char('r') => DebugAction::RunToCursor,
        KeyCode::Named(FenixNamedKey::Escape) => DebugAction::Leave,
        _ => return None,
    })
}

/// The F-keys, everyone else's debugger keys, in every mode: F5 starts or
/// continues, Shift-F5 stops, Ctrl-Shift-F5 restarts, F6 pauses, F9 sets
/// a breakpoint, F10 / F11 / Shift-F11 step over, in and out.
pub(super) fn function_key_action(key: &winit::keyboard::NamedKey, shift: bool, ctrl: bool) -> Option<DebugAction> {
    use winit::keyboard::NamedKey as K;
    Some(match (key, shift, ctrl) {
        (K::F5, false, false) => DebugAction::StartOrContinue,
        (K::F5, true, false) => DebugAction::Stop,
        (K::F5, true, true) => DebugAction::Restart,
        (K::F6, false, false) => DebugAction::Pause,
        (K::F9, false, false) => DebugAction::ToggleBreakpoint,
        (K::F10, false, false) => DebugAction::StepOver,
        (K::F11, false, false) => DebugAction::StepIn,
        (K::F11, true, false) => DebugAction::StepOut,
        _ => return None,
    })
}

/// The debug dock's height in rows: its header, the columns' titles, ten
/// rows of each and the evaluate line.
pub(super) const DEBUG_DOCK_ROWS: usize = 13;

/// The dock's height in pixels, at `line_height`.
pub(crate) fn debug_dock_height(line_height: f32) -> f32 {
    DEBUG_DOCK_ROWS as f32 * line_height + crate::text::PAD_TOP * 2.0
}

/// Columns side by side in `width` characters, each `(title, lines,
/// share)` given its share of the width, its lines cut or padded to it
/// and cut at `rows`: the rows, each column's text apart, the titles
/// first.
pub(super) fn dock_columns(columns: &[(&str, Vec<String>, usize)], width: usize, rows: usize) -> Vec<Vec<String>> {
    let total: usize = columns.iter().map(|(_, _, share)| share).sum::<usize>().max(1);
    let gaps = columns.len().saturating_sub(1) * 3;
    let usable = width.saturating_sub(gaps);
    let widths: Vec<usize> = columns.iter().map(|(_, _, share)| usable * share / total).collect();
    let fit = |text: &str, w: usize| -> String {
        let n = text.chars().count();
        if n > w {
            let mut cut: String = text.chars().take(w.saturating_sub(1)).collect();
            cut.push('…');
            cut
        } else {
            format!("{text}{}", " ".repeat(w - n))
        }
    };
    (0..rows)
        .map(|row| {
            columns
                .iter()
                .zip(&widths)
                .map(|((title, lines, _), &w)| match row {
                    0 => fit(title, w),
                    r => fit(lines.get(r - 1).map(String::as_str).unwrap_or(""), w),
                })
                .collect()
        })
        .collect()
}

/// DEBUG's keys, as the modeline shows them.
const DEBUG_KEYS: &str = "n next  s in  o out  c continue  r to cursor  Esc normal";

/// What the gutter shows beside a line while debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DebugMark {
    Breakpoint(BreakpointLook),
    /// Where the selected frame is stopped.
    Stop,
    /// Stopped on a line that has a breakpoint.
    StopOnBreakpoint(BreakpointLook),
}

/// Whose decorations the inline values are, beside the extensions'.
pub(super) const DEBUG_DECORATIONS: &str = "fenix.debug";

/// How far above the stop values are written: a function's worth.
const INLINE_LOOKBACK: usize = 40;

/// The longest value written inline; the rest is in the panel.
const INLINE_VALUE_CHARS: usize = 48;

/// `path`'s file name alone, for a narrow column.
fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

/// Whether `a` and `b` name the same file, as a breakpoint's path, a
/// buffer's and an adapter's stack frame give them: separators and, on
/// Windows, letter case aside.
pub(super) fn same_file(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = fenix_lsp::normalize(p.to_path_buf()).to_string_lossy().replace('\\', "/");
        if cfg!(windows) { s.to_lowercase() } else { s }
    };
    norm(a) == norm(b)
}

/// The values to write at the ends of lines, for a frame stopped at
/// `stop_line` (0-based): each local whose name appears as a word on a line
/// from `INLINE_LOOKBACK` above the stop down to it, on the last such line
/// only, `name = value` with the line's names in the order they appear.
/// `lines` is `(0-based line, text)` for those lines, in order.
#[cfg(test)]
pub(super) fn inline_values(lines: &[(usize, String)], stop_line: usize, locals: &[(String, String)]) -> Vec<(usize, String)> {
    inline_pieces(lines, stop_line, locals).into_iter().map(|(line, pieces)| (line, pieces.into_iter().map(|(n, v)| format!("{n} = {v}")).collect::<Vec<_>>().join("  "))).collect()
}

/// `inline_values`, each line's `(name, value)`s apart, the value cut to
/// fit.
pub(super) fn inline_pieces(lines: &[(usize, String)], stop_line: usize, locals: &[(String, String)]) -> Vec<(usize, Vec<(String, String)>)> {
    let first = stop_line.saturating_sub(INLINE_LOOKBACK);
    let lines: Vec<&(usize, String)> = lines.iter().filter(|(l, _)| (first..=stop_line).contains(l)).collect();
    let words = |text: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in text.chars().chain(std::iter::once(' ')) {
            if c.is_alphanumeric() || c == '_' {
                cur.push(c);
            } else if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        out
    };
    // Each local's last line, at or above the stop.
    let mut last: HashMap<&str, usize> = HashMap::new();
    for (line, text) in &lines {
        for w in words(text) {
            if let Some((name, _)) = locals.iter().find(|(n, _)| *n == w) {
                last.insert(name.as_str(), *line);
            }
        }
    }
    let mut out = Vec::new();
    for (line, text) in &lines {
        let mut shown: Vec<(String, String)> = Vec::new();
        for w in words(text) {
            if last.get(w.as_str()) == Some(line) && !shown.iter().any(|(n, _)| *n == w) {
                if let Some((name, value)) = locals.iter().find(|(n, _)| *n == w) {
                    shown.push((name.clone(), cut_value(value)));
                }
            }
        }
        if !shown.is_empty() {
            out.push((*line, shown));
        }
    }
    out
}

/// A value as it's written inline: no control characters, cut at
/// `INLINE_VALUE_CHARS`.
fn cut_value(value: &str) -> String {
    let value: String = value.chars().filter(|c| !c.is_control()).collect();
    if value.chars().count() > INLINE_VALUE_CHARS { format!("{}…", value.chars().take(INLINE_VALUE_CHARS - 1).collect::<String>()) } else { value }
}

/// A line's values as decorations at `at`, the end of the line: each
/// `name = value`, a changed one's value in ember with what it was struck
/// beside it (`changed`).
fn value_decorations(at: usize, pieces: &[(String, String)], changed: &[(String, String)]) -> Vec<crate::decorations::Decoration> {
    use crate::decorations::{Decoration, Look};
    use crate::page::Role;
    let text = |text: String, role: Role| Decoration { start: at, end: at, look: Look::Text { text, role, after: true }, id: None };
    let mut out = Vec::new();
    let mut lead = "    ".to_string();
    for (name, value) in pieces {
        match changed.iter().find(|(n, _)| n == name) {
            Some((_, old)) => {
                out.push(text(format!("{lead}{name} = "), Role::Muted));
                out.push(text(value.clone(), Role::Warn));
                out.push(text(" ".to_string(), Role::Muted));
                out.push(Decoration { start: at, end: at, look: Look::Struck(cut_value(old)), id: None });
            }
            None => out.push(text(format!("{lead}{name} = {value}"), Role::Muted)),
        }
        lead = "  ".to_string();
    }
    out
}

/// Where the session is: starting, running, stopped where, or ended.
pub(super) fn debug_state_text(session: &DebugSession) -> String {
    if session.ended {
        return "ended".to_string();
    }
    // Stopped in a child session: which.
    let child = session.children.iter().find(|c| c.generation == session.stopped_on).map(|c| format!(" in {}", c.name)).unwrap_or_default();
    match (session.stopped_thread, session.call_stack.first()) {
        (Some(_), Some((function, path, line))) => format!("stopped {function} {}:{line}{child}", file_name(path)),
        (Some(_), None) => "stopped".to_string(),
        (None, _) if session.initialized => "running".to_string(),
        (None, _) => "starting".to_string(),
    }
}

impl App {
    /// Whether the editor is in DEBUG: a session stopped in the project
    /// you're in, Normal mode in the editor, and not left with Esc since
    /// it stopped.
    pub(super) fn debug_mode_active(&self) -> bool {
        let Some(session) = self.debug_session.as_ref() else { return false };
        session.stopped_thread.is_some()
            && !session.ended
            && !self.debug_mode_left
            && self.vim.mode() == Mode::Normal
            && self.main_view == MainView::Editor
            && !self.sidebar_focused
            && self.wasm.captured.is_none()
            && session.root == self.integration_root()
    }

    /// A key in DEBUG, done if it's one of DEBUG's own; false for the rest,
    /// and for a key that's part of something under way (a leader key, a
    /// count, an operator).
    pub(super) fn debug_mode_key(&mut self, key: KeyPress) -> bool {
        if !self.debug_mode_active() || self.leader_matcher.is_pending() || self.local_matcher.is_some() || !self.vim.is_idle() {
            return false;
        }
        let Some(action) = debug_mode_action(key) else { return false };
        self.debug_do(action);
        true
    }

    /// An F-key, in any mode: done if it's a debugger key.
    pub(super) fn debug_function_key(&mut self, key: &winit::keyboard::NamedKey) -> bool {
        let Some(action) = function_key_action(key, self.modifiers.shift_key(), self.modifiers.control_key()) else { return false };
        self.debug_do(action);
        true
    }

    pub(super) fn debug_do(&mut self, action: DebugAction) {
        match action {
            DebugAction::StartOrContinue => self.debug_start_or_continue(),
            DebugAction::StepOver => self.debug_step_over(),
            DebugAction::StepIn => self.debug_step_into(),
            DebugAction::StepOut => self.debug_step_out(),
            DebugAction::RunToCursor => self.debug_run_to_cursor(),
            DebugAction::Pause => self.debug_pause(),
            DebugAction::Stop => self.debug_stop(),
            DebugAction::Restart => self.debug_restart(),
            DebugAction::ToggleBreakpoint => self.debug_toggle_breakpoint(),
            DebugAction::Leave => self.debug_mode_left = true,
        }
    }

    /// `r` in DEBUG: on to the cursor's line, as a breakpoint there that
    /// lasts until it next stops, wherever that is.
    pub(crate) fn debug_run_to_cursor(&mut self) {
        let Some(path) = self.open().buffer.path().map(Path::to_path_buf) else {
            self.set_error("buffer has no file to run to".to_string());
            return;
        };
        if self.debug_session.as_ref().and_then(|s| s.stopped_thread).is_none() {
            self.set_error("debug session isn't stopped".to_string());
            return;
        }
        let (line, _) = self.open().buffer.line_col(&self.cursor());
        let canonical = fenix_lsp::normalize(std::fs::canonicalize(&path).unwrap_or(path));
        if let Some((old, _)) = self.debug_run_to.replace((canonical.clone(), line + 1)) {
            if old != canonical {
                self.send_set_breakpoints_for(&old);
            }
        }
        self.send_set_breakpoints_for(&canonical);
        self.debug_start_or_continue();
    }

    /// The run-to-cursor breakpoint, gone once it has stopped somewhere.
    pub(super) fn debug_run_to_reached(&mut self) {
        if let Some((path, _)) = self.debug_run_to.take() {
            self.send_set_breakpoints_for(&path);
        }
    }

    /// F6: stops the running program where it is.
    pub(crate) fn debug_pause(&mut self) {
        if !self.debug_scope_matches() { return; }
        let Some(session) = self.debug_session.as_ref().filter(|s| !s.ended) else {
            self.set_error("no debug session running".to_string());
            return;
        };
        if session.stopped_thread.is_some() {
            self.set_error("debug session is already stopped".to_string());
            return;
        }
        // The connection the program runs on: a child's, under js-debug.
        let thread_id = session.last_thread.unwrap_or(0);
        if let Some(client) = session.client_of(session.running_on).or_else(|| session.client_of(session.generation)) {
            let _ = client.request(fenix_dap::requests::Request::Pause(fenix_dap::requests::PauseRequestArguments::builder().thread_id(thread_id).build()));
        }
    }

    /// Ctrl-Shift-F5: the session ended and started again, as it was
    /// launched -- the same adapter, program and attributes.
    pub(crate) fn debug_restart(&mut self) {
        if !self.debug_scope_matches() { return; }
        let Some(session) = self.debug_session.as_ref() else {
            self.set_error("no debug session to restart".to_string());
            return;
        };
        let (language, root, adapter, attributes, attach, name) = (session.language, session.root.clone(), session.adapter.clone(), session.launch_attributes.clone(), session.attach, session.name.clone());
        if !session.ended || !session.client_closed {
            self.debug_stop();
        }
        self.debug_clear_ended();
        self.start_debug_session(language, root, adapter, attributes, attach, name);
    }

    /// The session on the modeline, from any pane: where it's stopped, or
    /// that it runs; in DEBUG, its keys too.
    pub(super) fn debug_modeline_segment(&self) -> String {
        if let Some(then) = self.debug_trail_modeline() {
            return format!("   [{then}]");
        }
        let Some(session) = self.debug_session.as_ref().filter(|s| !s.ended) else { return String::new() };
        let state = debug_state_text(session);
        if self.debug_mode_active() { format!("   [{state}]   {DEBUG_KEYS}") } else { format!("   [{state}]") }
    }

    /// Whether the debug dock is shown: there's a session, and it hasn't
    /// been put away (`SPC u d`).
    pub(super) fn debug_dock_shown(&self) -> bool {
        self.debug_dock_open && self.debug_session.is_some()
    }

    /// The dock's height, nothing while it's put away.
    pub(super) fn debug_dock_px(&self) -> f32 {
        if !self.debug_dock_shown() {
            return 0.0;
        }
        debug_dock_height(self.text.as_ref().map(|t| t.line_height()).unwrap_or(crate::text::LINE_HEIGHT))
    }

    /// `SPC u d`: the dock shown, or put away.
    pub(crate) fn debug_toggle_dock(&mut self) {
        if self.debug_session.is_none() {
            self.set_error("no debug session -- SPC u u or F5 starts one".to_string());
            return;
        }
        self.debug_dock_open = !self.debug_dock_open;
        self.debug_drawer.focused &= self.debug_dock_open;
    }

    /// Where it's stopped: the top frame's file and 1-based line.
    pub(super) fn debug_stop_location(&self) -> Option<(PathBuf, usize)> {
        self.debug_frame_location(0)
    }

    /// Where the `i`th frame is: its file and 1-based line.
    fn debug_frame_location(&self, i: usize) -> Option<(PathBuf, usize)> {
        let session = self.debug_session.as_ref()?;
        session.stopped_thread?;
        let (_, path, line) = session.call_stack.get(i)?;
        (!path.as_os_str().is_empty() && *line > 0).then(|| (path.clone(), *line))
    }

    /// The gutter's debug marks for `buffer` in a pane showing the 0-based
    /// document lines `lines` from its first row, `rows` of them: each
    /// `(row, mark)`. Nothing for a buffer without a file, or when there's
    /// nothing to show.
    pub(super) fn debug_marks(&self, buffer: BufferId, lines: &[usize], rows: usize) -> Vec<(usize, DebugMark)> {
        let stop = self.debug_stop_location();
        if self.breakpoints.values().all(|l| l.is_empty()) && stop.is_none() {
            return Vec::new();
        }
        let Some(path) = self.buffers.get(buffer).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else { return Vec::new() };
        let breakpoints: HashMap<usize, BreakpointLook> = self.breakpoints.iter().filter(|(p, _)| same_file(p, &path)).flat_map(|(_, lines)| lines.iter().map(|(l, b)| (*l, b.look()))).collect();
        let stop_line = stop.filter(|(p, _)| same_file(p, &path)).map(|(_, line)| line);
        let mut marks = Vec::new();
        for (row, &line) in lines.iter().enumerate().take(rows + 1) {
            let one_based = line + 1;
            let mark = match (stop_line == Some(one_based), breakpoints.get(&one_based)) {
                (true, Some(look)) => DebugMark::StopOnBreakpoint(*look),
                (true, None) => DebugMark::Stop,
                (false, Some(look)) => DebugMark::Breakpoint(*look),
                (false, None) => continue,
            };
            marks.push((row, mark));
        }
        marks
    }

    /// The selected frame's values written at the ends of its lines, in
    /// every buffer on its file; taken away from them all when it isn't
    /// stopped. Called whenever the debug panes are drawn again.
    pub(super) fn refresh_debug_decorations(&mut self) {
        let mut stale: Vec<BufferId> = self.wasm.decorations.iter().filter(|(_, sets)| sets.iter().any(|(who, _)| who == DEBUG_DECORATIONS)).map(|(id, _)| *id).collect();
        // A stop gone back to along the trail, its values as they were; else
        // the selected frame's, and what changed at this stop.
        let (stop, locals, changed) = match self.debug_trail_viewed() {
            Some(then) => (Some((then.path.clone(), then.line)), then.values.clone(), then.changed.clone()),
            None => {
                let selected = self.debug_session.as_ref().map(|s| s.selected_frame).unwrap_or(0);
                let locals: Vec<(String, String)> = self
                    .debug_session
                    .as_ref()
                    .filter(|s| s.stopped_thread.is_some())
                    .and_then(|s| s.variables.first().map(|scope| scope.values()))
                    .unwrap_or_default();
                (self.debug_frame_location(selected), locals, self.debug_trail_changed_now())
            }
        };
        // Each buffer's: the values at its stop, and its pins.
        let mut found: HashMap<BufferId, Vec<crate::decorations::Decoration>> = HashMap::new();
        if let Some((path, line)) = stop.filter(|_| !locals.is_empty()) {
            let ids: Vec<BufferId> = self.buffers.ids_sorted_by_path().into_iter().filter(|id| self.buffers.get(*id).and_then(|ob| ob.buffer.path()).is_some_and(|p| same_file(p, &path))).collect();
            for id in ids {
                let Some(ob) = self.buffers.get(id) else { continue };
                let stop_line = line - 1;
                if stop_line >= ob.buffer.line_count() {
                    continue;
                }
                let first = stop_line.saturating_sub(INLINE_LOOKBACK);
                let lines: Vec<(usize, String)> = (first..=stop_line).map(|l| (l, ob.buffer.line(l).to_string())).collect();
                found.entry(id).or_default().extend(inline_pieces(&lines, stop_line, &locals).into_iter().flat_map(|(l, pieces)| value_decorations(ob.buffer.line_start_char(l) + ob.buffer.line_len(l), &pieces, &changed)));
            }
        }
        if !self.debug_pins.is_empty() && self.debug_session.is_some() {
            for id in self.buffers.ids_sorted_by_path() {
                let Some(ob) = self.buffers.get(id) else { continue };
                let Some(path) = ob.buffer.path() else { continue };
                let pins = self.debug_pin_decorations(path, &ob.buffer);
                if !pins.is_empty() {
                    found.entry(id).or_default().extend(pins);
                }
            }
        }
        for (id, decorations) in found {
            stale.retain(|s| *s != id);
            let sets = self.wasm.decorations.entry(id).or_default();
            sets.retain(|(who, _)| who != DEBUG_DECORATIONS);
            sets.push((DEBUG_DECORATIONS.to_string(), decorations));
        }
        for id in stale {
            if let Some(sets) = self.wasm.decorations.get_mut(&id) {
                sets.retain(|(who, _)| who != DEBUG_DECORATIONS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dock_lays_its_columns_side_by_side_cut_to_their_share() {
        let rows = dock_columns(&[("STACK", vec!["main  app.py:3".to_string()], 1), ("VARS", vec!["a = 1".to_string(), "a_very_long_name = 2".to_string()], 1)], 23, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ["STACK     ", "VARS      "], "23 less one gap of 3, halved");
        assert_eq!(rows[1], ["main  app…", "a = 1     "]);
        assert_eq!(rows[2], ["          ", "a_very_lo…"]);
    }

    #[test]
    fn debug_mode_has_five_keys_and_esc_and_the_f_keys_are_the_usual_ones() {
        use DebugAction::*;
        let keys: Vec<Option<DebugAction>> = "nsocrjx".chars().map(|c| debug_mode_action(KeyPress::char(c))).collect();
        assert_eq!(keys, [Some(StepOver), Some(StepIn), Some(StepOut), Some(StartOrContinue), Some(RunToCursor), None, None]);
        assert_eq!(debug_mode_action(KeyPress::named(FenixNamedKey::Escape)), Some(Leave));
        assert_eq!(debug_mode_action(KeyPress::char('n').with_ctrl()), None, "Ctrl-n is Normal's");
        use winit::keyboard::NamedKey as K;
        assert_eq!(function_key_action(&K::F5, false, false), Some(StartOrContinue));
        assert_eq!(function_key_action(&K::F5, true, false), Some(Stop));
        assert_eq!(function_key_action(&K::F5, true, true), Some(Restart));
        assert_eq!(function_key_action(&K::F11, true, false), Some(StepOut));
        assert_eq!(function_key_action(&K::F12, false, false), None);
    }

    fn lines(text: &str) -> Vec<(usize, String)> {
        text.lines().enumerate().map(|(i, l)| (i, l.to_string())).collect()
    }

    #[test]
    fn each_local_is_written_once_on_its_last_line_above_the_stop() {
        let src = lines("def build_rows(orders, rate):\n    rows = []\n    for o in orders:\n        total = o.qty * rate\n        rows.append(total)\n");
        let locals = vec![("orders".to_string(), "[Order ×38]".to_string()), ("rate".to_string(), "1.36".to_string()), ("o".to_string(), "Order(1042)".to_string()), ("rows".to_string(), "[]".to_string()), ("total".to_string(), "4.08".to_string())];
        let got = inline_values(&src, 3, &locals);
        assert_eq!(got, vec![(1, "rows = []".to_string()), (2, "orders = [Order ×38]".to_string()), (3, "total = 4.08  o = Order(1042)  rate = 1.36".to_string())], "nothing below the stop; orders and rate on their last lines");
    }

    #[test]
    fn a_long_value_is_cut_and_only_whole_words_match() {
        let src = lines("ratio = rate_of(x)\n");
        let long = "x".repeat(80);
        let got = inline_values(&src, 0, &[("rate".to_string(), "1".to_string()), ("x".to_string(), long)]);
        assert_eq!(got.len(), 1);
        assert!(got[0].1.starts_with("x = xxx") && got[0].1.ends_with('…') && !got[0].1.contains("rate ="), "{:?}", got);
    }

    #[test]
    fn paths_compare_across_separators_and_case_on_windows() {
        assert!(same_file(Path::new("C:/p/src/main.rs"), Path::new("C:\\p\\src\\main.rs")));
        assert_eq!(same_file(Path::new("C:/P/Main.rs"), Path::new("c:/p/main.rs")), cfg!(windows));
    }
}
