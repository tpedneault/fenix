//! The Ember Trail (the debugger overhaul's third phase): where the
//! program has been, kept stop by stop. Each stop is a snapshot -- where,
//! in what, and the top frame's values -- the last `debug.trail_stops` of
//! them (200 unless set). The gutter shows the lines it stopped on as a
//! stripe that cools a step at a time; a value that changed since the
//! stop before is written in ember, the old one struck beside it. The
//! ribbon under the focused pane's tabs is the trail itself: `[t` and `]t`
//! go back and forth along it, the code and its values as they were then,
//! and Enter comes back to now.
//!
//! The trail lives in Fenix, not the session: a restarted session keeps
//! the one before, dimmed. Nothing of it is written to disk, so a new
//! Fenix starts without one.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use fenix_keymap::{KeyPress, NamedKey as FenixNamedKey};

use super::debug_host::same_file;
use super::*;

/// The setting that says how many stops the trail keeps.
pub(super) const TRAIL_SETTING: &str = "debug.trail_stops";

/// How many it keeps when that isn't set.
pub(super) const DEFAULT_TRAIL_STOPS: usize = 200;

/// How much a stop's stripe cools each step after it.
const COOLING: f32 = 0.85;

/// A stripe cooler than this isn't drawn: about seventeen steps back.
const COLD: f32 = 0.06;

/// How bright a session before this one's stripe is.
const OLD_TRAIL: f32 = 0.3;

/// The most stops the ribbon shows.
const RIBBON_STOPS: usize = 48;

/// One stop, as it was.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StopSnapshot {
    /// The session it was in, by its generation.
    pub session: u64,
    /// The project it was in.
    pub root: PathBuf,
    pub path: PathBuf,
    /// 1-based.
    pub line: usize,
    pub function: String,
    /// The top frame's first scope's values, once they've come.
    pub values: Vec<(String, String)>,
    /// Those of `values` that weren't so at the stop before in the same
    /// function: each name, and what it was.
    pub changed: Vec<(String, String)>,
}

/// The stops so far, the oldest first, and the one being looked back at.
#[derive(Debug, Default)]
pub(super) struct EmberTrail {
    pub stops: VecDeque<StopSnapshot>,
    /// An index into `stops` while `[t` has gone back; None is now.
    pub viewing: Option<usize>,
}

/// What's changed from `before` to `after`: each name in both whose value
/// isn't the same, with what it was.
pub(super) fn changed_values(before: &[(String, String)], after: &[(String, String)]) -> Vec<(String, String)> {
    after
        .iter()
        .filter_map(|(name, value)| before.iter().find(|(n, _)| n == name).filter(|(_, old)| old != value).map(|(_, old)| (name.clone(), old.clone())))
        .collect()
}

/// How warm a stop `age` steps back is, 1 the newest: none once it's cold.
pub(super) fn warmth(age: usize) -> Option<f32> {
    let w = COOLING.powi(age.min(i32::MAX as usize) as i32);
    (w >= COLD).then_some(w)
}

impl EmberTrail {
    /// A new stop, the oldest gone past `keep`; back to now.
    pub fn push(&mut self, stop: StopSnapshot, keep: usize) {
        self.stops.push_back(stop);
        while self.stops.len() > keep.max(1) {
            self.stops.pop_front();
        }
        self.viewing = None;
    }

    /// The newest stop's values, come: what changed since the stop before
    /// it in the same function and session worked out.
    pub fn fill(&mut self, session: u64, values: Vec<(String, String)>) {
        let Some(newest) = self.stops.len().checked_sub(1) else { return };
        if self.stops[newest].session != session {
            return;
        }
        let (path, function) = (self.stops[newest].path.clone(), self.stops[newest].function.clone());
        let before = self.stops.iter().take(newest).rev().find(|s| s.session == session && s.function == function && s.path == path && !s.values.is_empty());
        let changed = before.map(|b| changed_values(&b.values, &values)).unwrap_or_default();
        let stop = &mut self.stops[newest];
        stop.values = values;
        stop.changed = changed;
    }

    /// The stop shown: the one gone back to, else the newest.
    pub fn shown(&self) -> Option<usize> {
        self.viewing.or(self.stops.len().checked_sub(1))
    }

    /// The lines of `path` stopped on up to the stop shown, each with how
    /// warm it is and whether it was this session (`session`) or one
    /// before. This session's warmth wins over an old one's.
    pub fn heat(&self, path: &Path, session: Option<u64>) -> HashMap<usize, (f32, bool)> {
        let mut out: HashMap<usize, (f32, bool)> = HashMap::new();
        let Some(upto) = self.shown() else { return out };
        for (age, stop) in self.stops.iter().take(upto + 1).rev().enumerate() {
            if !same_file(&stop.path, path) {
                continue;
            }
            let this = Some(stop.session) == session;
            let w = if this { warmth(age) } else { Some(OLD_TRAIL) };
            let Some(w) = w else { continue };
            let entry = out.entry(stop.line).or_insert((w, this));
            if (this && !entry.1) || (this == entry.1 && w > entry.0) {
                *entry = (w, this);
            }
        }
        out
    }
}

impl App {
    /// How many stops the trail keeps (`debug.trail_stops`).
    fn debug_trail_keep(&self) -> usize {
        self.config.int_value(TRAIL_SETTING).and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0).unwrap_or(DEFAULT_TRAIL_STOPS)
    }

    /// A stop, its stack come: on the trail, its values to follow. A
    /// session in another project starts the trail again.
    pub(super) fn debug_trail_stopped(&mut self) {
        let Some(session) = self.debug_session.as_ref() else { return };
        let Some((function, path, line)) = session.call_stack.first().filter(|(_, p, l)| !p.as_os_str().is_empty() && *l > 0).cloned() else { return };
        let (id, root) = (session.generation, session.root.clone());
        if self.debug_trail.stops.back().is_some_and(|s| s.root != root) {
            self.debug_trail.stops.clear();
        }
        let keep = self.debug_trail_keep();
        self.debug_trail.push(StopSnapshot { session: id, root, path, line, function, values: Vec::new(), changed: Vec::new() }, keep);
    }

    /// The top frame's first scope's values, come: the newest stop's.
    pub(super) fn debug_trail_values(&mut self, values: Vec<(String, String)>) {
        let Some(id) = self.debug_session.as_ref().map(|s| s.generation) else { return };
        self.debug_trail.fill(id, values);
    }

    /// Whether `[t` and `]t` are the trail's: there's a session in the
    /// project you're in, and a trail to go along. They're the TODOs'
    /// otherwise.
    pub(super) fn debug_trail_here(&self) -> bool {
        let root = self.integration_root();
        self.debug_session.as_ref().is_some_and(|s| s.root == root) && !self.debug_trail.stops.is_empty()
    }

    /// `[t` (back) and `]t` (forward), `count` stops: the code and its
    /// values as they were then. Forward past the newest is now.
    pub(super) fn debug_trail_scrub(&mut self, forward: bool, count: usize) {
        let len = self.debug_trail.stops.len();
        let Some(at) = self.debug_trail.shown() else { return };
        let to = if forward { at + count } else { at.saturating_sub(count) };
        if to + 1 >= len {
            self.debug_trail_now();
            return;
        }
        self.debug_trail.viewing = Some(to);
        let stop = &self.debug_trail.stops[to];
        let (path, line) = (stop.path.clone(), stop.line);
        self.goto_debug_location(&path, line);
        self.refresh_debug_panes();
    }

    /// Back to now: the trail's end, and the code where it's stopped.
    pub(super) fn debug_trail_now(&mut self) {
        if self.debug_trail.viewing.take().is_none() {
            return;
        }
        if let Some((path, line)) = self.debug_stop_location() {
            self.goto_debug_location(&path, line);
        }
        self.refresh_debug_panes();
    }

    /// Enter while gone back along the trail, in Normal: back to now.
    pub(super) fn debug_trail_key(&mut self, key: KeyPress) -> bool {
        if self.debug_trail.viewing.is_none() || key != KeyPress::named(FenixNamedKey::Enter) {
            return false;
        }
        if self.vim.mode() != Mode::Normal || !self.vim.is_idle() || self.main_view != MainView::Editor || self.sidebar_focused {
            return false;
        }
        self.debug_trail_now();
        true
    }

    /// The stop gone back to, while one is: what the code shows instead
    /// of now.
    pub(super) fn debug_trail_viewed(&self) -> Option<&StopSnapshot> {
        self.debug_trail.viewing.and_then(|i| self.debug_trail.stops.get(i))
    }

    /// What changed at the stop now, when the values written in the code
    /// are its: the top frame selected, stopped where the trail's newest
    /// stop is.
    pub(super) fn debug_trail_changed_now(&self) -> Vec<(String, String)> {
        let Some(session) = self.debug_session.as_ref().filter(|s| s.selected_frame == 0) else { return Vec::new() };
        let Some(newest) = self.debug_trail.stops.back().filter(|s| s.session == session.generation) else { return Vec::new() };
        match self.debug_stop_location() {
            Some((path, line)) if same_file(&path, &newest.path) && line == newest.line => newest.changed.clone(),
            _ => Vec::new(),
        }
    }

    /// The gutter's stripe for `buffer`, in a pane showing the 0-based
    /// document lines `lines` from its first row: each `(row, colour)`,
    /// this session's in ember as warm as it is, one before's dim.
    pub(super) fn debug_trail_marks(&self, buffer: BufferId, lines: &[usize], rows: usize) -> Vec<(usize, [f32; 4])> {
        if self.debug_trail.stops.is_empty() {
            return Vec::new();
        }
        let Some(path) = self.buffers.get(buffer).and_then(|ob| ob.buffer.path()).map(Path::to_path_buf) else { return Vec::new() };
        let heat = self.debug_trail.heat(&path, self.debug_session.as_ref().map(|s| s.generation));
        if heat.is_empty() {
            return Vec::new();
        }
        let [er, eg, eb, _] = glyphon_to_rgba(self.theme.git_modified);
        let [mr, mg, mb, _] = glyphon_to_rgba(self.theme.gutter_fg);
        lines
            .iter()
            .enumerate()
            .take(rows + 1)
            .filter_map(|(row, line)| heat.get(&(line + 1)).map(|&(w, this)| (row, if this { [er, eg, eb, w] } else { [mr, mg, mb, w] })))
            .collect()
    }

    /// Whether the ribbon is under the focused pane's tabs, in place of
    /// its breadcrumb: there's a trail here.
    pub(super) fn debug_ribbon_shown(&self) -> bool {
        self.debug_trail_here()
    }

    /// The ribbon: the trail's newest stops, the one shown marked, this
    /// session's in ember and those before dim; then where the one shown
    /// is, and the keys.
    pub(super) fn debug_ribbon_spans(&self) -> Option<Vec<(String, glyphon::Color, bool)>> {
        if !self.debug_ribbon_shown() {
            return None;
        }
        let trail = &self.debug_trail;
        let shown = trail.shown()?;
        let session = self.debug_session.as_ref().map(|s| s.generation);
        let theme = &self.theme;
        let (ember, muted, text) = (theme.git_modified, theme.gutter_fg, theme.fg_modeline);
        let len = trail.stops.len();
        let first = len.saturating_sub(RIBBON_STOPS).min(shown.saturating_sub(RIBBON_STOPS / 2));
        let mut out = vec![(" ".to_string(), text, false)];
        if first > 0 {
            out.push(("… ".to_string(), muted, false));
        }
        for (i, stop) in trail.stops.iter().enumerate().skip(first).take(RIBBON_STOPS) {
            let this = Some(stop.session) == session;
            let (glyph, color) = match (i == shown, this) {
                (true, _) => ("◉", ember),
                (false, true) => ("●", ember),
                (false, false) => ("·", muted),
            };
            out.push((glyph.to_string(), color, false));
        }
        let stop = &trail.stops[shown];
        let name = stop.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push((format!("   stop {}/{len}  {} {name}:{}", shown + 1, stop.function, stop.line), text, false));
        if !stop.changed.is_empty() {
            let names: Vec<&str> = stop.changed.iter().map(|(n, _)| n.as_str()).collect();
            out.push((format!("  changed {}", names.join(" ")), ember, false));
        }
        let keys = if trail.viewing.is_some() { "   [t back  ]t on  Enter now" } else { "   [t goes back along the trail" };
        out.push((keys.to_string(), muted, true));
        Some(out)
    }

    /// Where the trail's gone back to, for the modeline: nothing at now.
    pub(super) fn debug_trail_modeline(&self) -> Option<String> {
        let at = self.debug_trail.viewing?;
        let stop = self.debug_trail.stops.get(at)?;
        let name = stop.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Some(format!("stop {}/{} {name}:{} -- Enter is now", at + 1, self.debug_trail.stops.len(), stop.line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(session: u64, file: &str, line: usize, function: &str) -> StopSnapshot {
        StopSnapshot { session, root: PathBuf::from("/p"), path: PathBuf::from(file), line, function: function.into(), values: Vec::new(), changed: Vec::new() }
    }

    fn vals(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_trail_keeps_its_newest_stops_and_a_stop_comes_back_to_now() {
        let mut trail = EmberTrail::default();
        for line in 1..=5 {
            trail.push(stop(1, "/p/a.py", line, "f"), 3);
        }
        assert_eq!(trail.stops.iter().map(|s| s.line).collect::<Vec<_>>(), [3, 4, 5]);
        trail.viewing = Some(0);
        trail.push(stop(1, "/p/a.py", 6, "f"), 3);
        assert_eq!(trail.viewing, None);
    }

    #[test]
    fn a_changed_value_is_one_that_was_otherwise_at_the_stop_before_in_its_function() {
        let mut trail = EmberTrail::default();
        trail.push(stop(1, "/p/a.py", 3, "f"), 10);
        trail.fill(1, vals(&[("total", "3.0"), ("i", "0")]));
        trail.push(stop(1, "/p/a.py", 9, "g"), 10);
        trail.fill(1, vals(&[("total", "99")]));
        assert!(trail.stops[1].changed.is_empty(), "another function's total isn't this one's");
        trail.push(stop(1, "/p/a.py", 4, "f"), 10);
        trail.fill(1, vals(&[("total", "4.08"), ("i", "0"), ("new", "1")]));
        assert_eq!(trail.stops[2].changed, vals(&[("total", "3.0")]));
        trail.fill(2, vals(&[("x", "1")]));
        assert_eq!(trail.stops[2].values.len(), 3, "another session's values aren't this stop's");
    }

    #[test]
    fn the_stripe_cools_a_step_at_a_time_and_an_old_sessions_is_dim() {
        let mut trail = EmberTrail::default();
        trail.push(stop(1, "/p/a.py", 1, "f"), 100);
        for line in [2, 3, 2] {
            trail.push(stop(2, "/p/a.py", line, "f"), 100);
        }
        trail.push(stop(2, "/p/b.py", 7, "g"), 100);
        let heat = trail.heat(Path::new("/p/a.py"), Some(2));
        assert_eq!(heat[&1], (OLD_TRAIL, false));
        assert_eq!(heat[&2], (COOLING, true), "its newest visit, one step back");
        assert_eq!(heat[&3], (COOLING * COOLING, true));
        trail.viewing = Some(2);
        let back = trail.heat(Path::new("/p/a.py"), Some(2));
        assert_eq!(back[&3], (1.0, true), "gone back to line 3: it's the warm one");
        assert_eq!(back[&2], (COOLING, true), "the visit to 2 after it isn't yet");
        assert_eq!(warmth(40), None);
    }

    #[test]
    fn what_changed_is_by_name() {
        let got = changed_values(&vals(&[("a", "1"), ("b", "2")]), &vals(&[("b", "3"), ("a", "1"), ("c", "9")]));
        assert_eq!(got, vals(&[("b", "2")]));
    }
}
