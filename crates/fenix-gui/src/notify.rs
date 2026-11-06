//! Notifications, in the manner of Dunst: toasts stacked in a corner of
//! the window, each from a source (`agents`, `tasks`, an extension's id),
//! with a summary, a body, an urgency, and actions -- the first one what
//! Enter or a click does. They time out by urgency (a critical one
//! stays until it's dismissed), only while Fenix is in front, so one
//! that came while you were away is still there when you're back. A
//! notification with the same source and tag as one showing replaces it
//! (a progress that moves on); the same summary and body again stacks
//! it, counted ("×3"). Every one is kept in the history (`SPC o n`) until
//! there are too many. Do not disturb sends everything but the critical
//! ones straight to the history; rules per source mute one, change its
//! urgency, or keep it to the history.
//!
//! This module is plain data: `App` (`app/notifications.rs`) feeds it the
//! clock, draws what `visible` gives, and runs what `act` hands back.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};

/// How many notifications the history keeps.
pub const HISTORY: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    pub fn parse(word: &str) -> Option<Urgency> {
        match word.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Urgency::Low),
            "normal" => Some(Urgency::Normal),
            "critical" => Some(Urgency::Critical),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Urgency::Low => "low",
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        }
    }
}

/// Something a notification offers to do: a command, by id, as the
/// palette runs it (`agents.open_thread:<id>` and the like are `App`'s
/// own, resolved by it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    pub command: String,
}

/// What's asked to be shown.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Request {
    /// Who it's from: `agents`, `tasks`, `fenix`, an extension's id.
    pub source: String,
    pub summary: String,
    pub body: String,
    pub urgency: Option<Urgency>,
    /// The first is the default: Enter, a click.
    pub actions: Vec<Action>,
    /// Replaces the one showing with the same source and tag.
    pub tag: Option<String>,
    /// How long it shows; none for its urgency's.
    pub timeout: Option<Duration>,
    /// A bar along its foot, 0 to 1.
    pub progress: Option<f32>,
    /// Only into the history, whatever else says: one past a source's
    /// rate.
    pub quiet: bool,
}

impl Request {
    pub fn new(source: impl Into<String>, summary: impl Into<String>) -> Request {
        Request { source: source.into(), summary: summary.into(), ..Request::default() }
    }

    pub fn body(mut self, body: impl Into<String>) -> Request {
        self.body = body.into();
        self
    }

    pub fn urgency(mut self, urgency: Urgency) -> Request {
        self.urgency = Some(urgency);
        self
    }

    pub fn action(mut self, label: impl Into<String>, command: impl Into<String>) -> Request {
        self.actions.push(Action { label: label.into(), command: command.into() });
        self
    }

    pub fn tag(mut self, tag: impl Into<String>) -> Request {
        self.tag = Some(tag.into());
        self
    }
}

/// One notification, shown or kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: u64,
    pub source: String,
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    pub actions: Vec<Action>,
    pub tag: Option<String>,
    pub progress: Option<f32>,
    /// How many times it came: the same summary and body again stacks.
    pub count: u32,
    /// When it came, for the history.
    pub at: SystemTime,
    /// How long it shows, counted only while Fenix is in front; none to
    /// stay.
    pub timeout: Option<Duration>,
    /// How much of `timeout` it has shown for.
    pub shown: Duration,
    /// Whether it was ever on screen, rather than straight to the
    /// history (do not disturb, a rule).
    pub seen: bool,
}

/// What a rule does with a source's notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Neither shown nor kept.
    Mute,
    /// Kept in the history, never shown.
    History,
    /// Shown at this urgency, whatever it asked for.
    Urgency(Urgency),
}

impl Rule {
    pub fn parse(word: &str) -> Option<Rule> {
        match word.trim().to_ascii_lowercase().as_str() {
            "mute" | "off" => Some(Rule::Mute),
            "history" | "quiet" => Some(Rule::History),
            other => Urgency::parse(other).map(Rule::Urgency),
        }
    }
}

/// The settings that shape what's shown: `[notifications]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    /// How many toasts at once; the rest wait.
    pub max_visible: usize,
    pub timeout_low: Duration,
    pub timeout_normal: Duration,
    /// None: until dismissed.
    pub timeout_critical: Option<Duration>,
    /// `(source, rule)`, a source's first match: its id, or a prefix
    /// ending in `*` (`fenix.*`).
    pub rules: Vec<(String, Rule)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            max_visible: 4,
            timeout_low: Duration::from_secs(4),
            timeout_normal: Duration::from_secs(8),
            timeout_critical: None,
            rules: Vec::new(),
        }
    }
}

impl Settings {
    fn rule(&self, source: &str) -> Option<Rule> {
        self.rules.iter().find(|(pattern, _)| match pattern.strip_suffix('*') {
            Some(prefix) => source.starts_with(prefix),
            None => pattern == source,
        }).map(|(_, rule)| *rule)
    }

    fn timeout(&self, urgency: Urgency) -> Option<Duration> {
        match urgency {
            Urgency::Low => Some(self.timeout_low),
            Urgency::Normal => Some(self.timeout_normal),
            Urgency::Critical => self.timeout_critical,
        }
    }
}

/// What `push` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pushed {
    /// A new toast, or one waiting its turn.
    Shown(u64),
    /// Into the one showing with its tag, or stacked on the same.
    Updated(u64),
    /// Only into the history: do not disturb, a rule, or notifications off.
    Kept(u64),
    /// Dropped by a rule.
    Muted,
}

#[derive(Debug, Default)]
pub struct Center {
    next_id: u64,
    /// Toasts showing or waiting their turn, oldest first.
    active: Vec<Note>,
    /// Every notification, newest last.
    history: VecDeque<Note>,
    /// Do not disturb: all but critical go to the history.
    pub dnd: bool,
    /// What came while in do not disturb, not yet looked at.
    pub missed: usize,
    /// The last time the clock was given, while in front.
    ticked: Option<Instant>,
}

impl Center {
    pub fn new() -> Center {
        Center::default()
    }

    pub fn push(&mut self, request: Request, settings: &Settings) -> Pushed {
        let rule = settings.rule(&request.source);
        if rule == Some(Rule::Mute) {
            return Pushed::Muted;
        }
        let urgency = match rule {
            Some(Rule::Urgency(u)) => u,
            _ => request.urgency.unwrap_or(Urgency::Normal),
        };
        let progress = request.progress.filter(|p| p.is_finite()).map(|p| p.clamp(0.0, 1.0));
        // The one showing with its tag, or the same again: updated in place.
        let same = self.active.iter().position(|n| match (&request.tag, &n.tag) {
            (Some(a), Some(b)) => a == b && n.source == request.source,
            (None, None) => n.source == request.source && n.summary == request.summary && n.body == request.body,
            _ => false,
        });
        if let Some(i) = same {
            let note = &mut self.active[i];
            if request.tag.is_none() {
                note.count += 1;
            }
            note.summary = request.summary;
            note.body = request.body;
            note.urgency = urgency;
            if !request.actions.is_empty() || request.tag.is_some() {
                note.actions = request.actions;
            }
            note.progress = progress;
            note.timeout = request.timeout.or(settings.timeout(urgency));
            note.shown = Duration::ZERO;
            note.at = SystemTime::now();
            let id = note.id;
            let copy = note.clone();
            if let Some(kept) = self.history.iter_mut().rev().find(|n| n.id == id) {
                *kept = copy;
            }
            return Pushed::Updated(id);
        }
        self.next_id += 1;
        let id = self.next_id;
        let quiet = request.quiet || !settings.enabled || rule == Some(Rule::History) || (self.dnd && urgency != Urgency::Critical);
        let note = Note {
            id,
            source: request.source,
            summary: request.summary,
            body: request.body,
            urgency,
            actions: request.actions,
            tag: request.tag,
            progress,
            count: 1,
            at: SystemTime::now(),
            timeout: request.timeout.or(settings.timeout(urgency)),
            shown: Duration::ZERO,
            seen: !quiet,
        };
        self.history.push_back(note.clone());
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
        if quiet {
            if self.dnd {
                self.missed += 1;
            }
            return Pushed::Kept(id);
        }
        self.active.push(note);
        Pushed::Shown(id)
    }

    /// The clock moves on, `focused` whether Fenix is in front: only then
    /// do the showing toasts' timeouts run. True when one went.
    pub fn tick(&mut self, now: Instant, focused: bool, settings: &Settings) -> bool {
        let last = self.ticked.replace(now);
        if !focused {
            self.ticked = None;
            return false;
        }
        let Some(last) = last else { return false };
        let step = now.saturating_duration_since(last);
        let showing = settings.max_visible.max(1);
        let before = self.active.len();
        let mut on_screen = 0;
        self.active.retain_mut(|note| {
            // Only those on screen count down; the rest wait their turn.
            let visible = on_screen < showing;
            on_screen += 1;
            if !visible {
                return true;
            }
            note.shown += step;
            note.timeout.is_none_or(|t| note.shown < t)
        });
        before != self.active.len()
    }

    /// When the next toast on screen times out, from `now`.
    pub fn next_deadline(&self, now: Instant, settings: &Settings) -> Option<Instant> {
        self.visible(settings).iter().filter_map(|n| n.timeout.map(|t| now + t.saturating_sub(n.shown))).min()
    }

    /// The toasts on screen, oldest first.
    pub fn visible(&self, settings: &Settings) -> &[Note] {
        let n = self.active.len().min(settings.max_visible.max(1));
        &self.active[..n]
    }

    /// How many wait their turn.
    pub fn waiting(&self, settings: &Settings) -> usize {
        self.active.len().saturating_sub(settings.max_visible.max(1))
    }

    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }

    pub fn history(&self) -> &VecDeque<Note> {
        &self.history
    }

    pub fn get(&self, id: u64) -> Option<&Note> {
        self.active.iter().find(|n| n.id == id).or_else(|| self.history.iter().find(|n| n.id == id))
    }

    /// The newest toast showing.
    pub fn newest(&self) -> Option<u64> {
        self.active.last().map(|n| n.id)
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.active.len();
        self.active.retain(|n| n.id != id);
        before != self.active.len()
    }

    /// The toast from `source` with `tag`.
    pub fn dismiss_tag(&mut self, source: &str, tag: &str) -> bool {
        let before = self.active.len();
        self.active.retain(|n| !(n.source == source && n.tag.as_deref() == Some(tag)));
        before != self.active.len()
    }

    pub fn dismiss_all(&mut self) -> usize {
        std::mem::take(&mut self.active).len()
    }

    /// Its action `index` (0, the default), and it's dismissed: the
    /// command to run.
    pub fn act(&mut self, id: u64, index: usize) -> Option<String> {
        let command = self.get(id)?.actions.get(index).map(|a| a.command.clone());
        self.dismiss(id);
        command
    }

    /// Shows one from the history again.
    pub fn reshow(&mut self, id: u64, settings: &Settings) -> bool {
        if self.active.iter().any(|n| n.id == id) {
            return false;
        }
        let Some(mut note) = self.history.iter().find(|n| n.id == id).cloned() else { return false };
        note.shown = Duration::ZERO;
        note.timeout = settings.timeout(note.urgency);
        note.seen = true;
        self.active.push(note);
        true
    }

    pub fn forget(&mut self, id: u64) {
        self.history.retain(|n| n.id != id);
        self.dismiss(id);
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Do not disturb on or off; off shows nothing of what was missed, but
    /// says how much there was.
    pub fn set_dnd(&mut self, on: bool) -> usize {
        self.dnd = on;
        if on { 0 } else { std::mem::take(&mut self.missed) }
    }
}

/// `[notifications] rules`, as the settings page keeps a map: source ->
/// rule. Ones that don't read are skipped, with why.
pub fn parse_rules(map: &[(String, String)]) -> (Vec<(String, Rule)>, Vec<String>) {
    let mut rules = Vec::new();
    let mut problems = Vec::new();
    for (source, rule) in map {
        match Rule::parse(rule) {
            Some(r) => rules.push((source.trim().to_string(), r)),
            None => problems.push(format!("notifications.rules: {source} = {rule}: mute, history, low, normal or critical")),
        }
    }
    (rules, problems)
}

/// How long ago, for the history: "now", "5 min", "2 h", "3 d".
pub fn ago(at: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(at).map(|d| d.as_secs()).unwrap_or(0);
    match secs {
        0..=44 => "now".into(),
        45..=3599 => format!("{} min", (secs + 30) / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        _ => format!("{} d", secs / 86_400),
    }
}

/// `text` wrapped to `width` characters, at most `max` lines, the last
/// ending in `…` when it was cut.
pub fn wrap(text: &str, width: usize, max: usize) -> Vec<String> {
    let width = width.max(8);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let word: String = if word.chars().count() > width { word.chars().take(width - 1).chain(['…']).collect() } else { word.to_string() };
            let needed = if line.is_empty() { word.chars().count() } else { line.chars().count() + 1 + word.chars().count() };
            if needed > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    if lines.len() > max {
        lines.truncate(max);
        if let Some(last) = lines.last_mut() {
            let mut cut: String = last.chars().take(width.saturating_sub(1)).collect();
            cut = cut.trim_end().to_string();
            cut.push('…');
            *last = cut;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn a_notification_shows_and_times_out_by_its_urgency_only_while_in_front() {
        let s = settings();
        let mut c = Center::new();
        let t0 = Instant::now();
        let Pushed::Shown(id) = c.push(Request::new("tasks", "build passed"), &s) else { panic!() };
        assert_eq!(c.visible(&s).len(), 1);
        c.tick(t0, true, &s);
        assert!(!c.tick(t0 + Duration::from_secs(5), true, &s), "8 s for a normal one");
        // Away for an hour: the clock doesn't run for it.
        c.tick(t0 + Duration::from_secs(5), false, &s);
        c.tick(t0 + Duration::from_secs(3605), true, &s);
        assert_eq!(c.newest(), Some(id), "still there when you're back");
        assert!(c.tick(t0 + Duration::from_secs(3609), true, &s), "and goes 8 s of being in front later");
        assert!(c.is_empty());
        assert_eq!(c.history().len(), 1, "but kept");
    }

    #[test]
    fn a_critical_one_stays_until_dismissed() {
        let s = settings();
        let mut c = Center::new();
        let t0 = Instant::now();
        let Pushed::Shown(id) = c.push(Request::new("agents", "permission").urgency(Urgency::Critical), &s) else { panic!() };
        c.tick(t0, true, &s);
        c.tick(t0 + Duration::from_secs(600), true, &s);
        assert_eq!(c.newest(), Some(id));
        assert_eq!(c.next_deadline(t0, &s), None);
        assert!(c.dismiss(id));
        assert!(c.is_empty());
    }

    #[test]
    fn the_same_again_stacks_and_a_tag_replaces() {
        let s = settings();
        let mut c = Center::new();
        let a = c.push(Request::new("ci", "main failed"), &s);
        assert_eq!(c.push(Request::new("ci", "main failed"), &s), match a { Pushed::Shown(id) => Pushed::Updated(id), _ => panic!() });
        assert_eq!(c.visible(&s)[0].count, 2);
        let Pushed::Shown(p) = c.push(Request { progress: Some(0.2), ..Request::new("tasks", "indexing").tag("index") }, &s) else { panic!() };
        assert_eq!(c.push(Request { progress: Some(0.7), ..Request::new("tasks", "indexing 70%").tag("index") }, &s), Pushed::Updated(p));
        let note = c.get(p).unwrap();
        assert_eq!((note.summary.as_str(), note.progress, note.count), ("indexing 70%", Some(0.7), 1));
        assert_eq!(c.visible(&s).len(), 2);
        assert_eq!(c.history().iter().find(|n| n.id == p).unwrap().summary, "indexing 70%", "the history has it as it is now");
    }

    #[test]
    fn past_max_visible_they_wait_their_turn_without_counting_down() {
        let s = Settings { max_visible: 2, ..settings() };
        let mut c = Center::new();
        let t0 = Instant::now();
        for i in 0..3 {
            c.push(Request::new("x", format!("n{i}")), &s);
        }
        assert_eq!((c.visible(&s).len(), c.waiting(&s)), (2, 1));
        c.tick(t0, true, &s);
        assert!(c.tick(t0 + Duration::from_secs(9), true, &s));
        let left: Vec<&str> = c.visible(&s).iter().map(|n| n.summary.as_str()).collect();
        assert_eq!(left, vec!["n2"], "the third, its 8 s only starting now");
        assert!(!c.tick(t0 + Duration::from_secs(16), true, &s));
        assert!(c.tick(t0 + Duration::from_secs(18), true, &s));
    }

    #[test]
    fn do_not_disturb_and_rules_send_them_to_the_history() {
        let s = Settings { rules: parse_rules(&[("fenix.pets".into(), "mute".into()), ("fenix.*".into(), "history".into()), ("agents".into(), "critical".into())]).0, ..settings() };
        let mut c = Center::new();
        assert_eq!(c.push(Request::new("fenix.pets", "nap"), &s), Pushed::Muted);
        assert!(matches!(c.push(Request::new("fenix.ci", "red"), &s), Pushed::Kept(_)));
        assert!(matches!(c.push(Request::new("agents", "done"), &s), Pushed::Shown(_)));
        assert_eq!(c.visible(&s)[0].urgency, Urgency::Critical);
        c.dismiss_all();
        c.set_dnd(true);
        assert!(matches!(c.push(Request::new("tasks", "done"), &s), Pushed::Kept(_)));
        assert!(matches!(c.push(Request::new("agents", "asks"), &s), Pushed::Shown(_)), "critical gets through");
        assert_eq!(c.set_dnd(false), 1, "one missed");
        assert_eq!(c.history().len(), 4);
        assert!(!c.history()[0].seen && c.history()[1].seen && !c.history()[2].seen);
        let (_, problems) = parse_rules(&[("x".into(), "loud".into())]);
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn acting_runs_the_default_action_and_dismisses() {
        let s = settings();
        let mut c = Center::new();
        let Pushed::Shown(id) = c.push(Request::new("agents", "turn ended").action("Open the thread", "agents.thread:7").action("Review", "agents.review"), &s) else { panic!() };
        assert_eq!(c.act(id, 1).as_deref(), Some("agents.review"));
        assert!(c.is_empty());
        assert!(c.reshow(id, &s));
        assert_eq!(c.act(id, 0).as_deref(), Some("agents.thread:7"));
        assert_eq!(c.act(id, 0), Some("agents.thread:7".into()), "from the history still");
        c.forget(id);
        assert!(c.get(id).is_none());
    }

    #[test]
    fn wrapping_and_ages_read_well() {
        assert_eq!(wrap("the quick brown fox jumps over the lazy dog", 15, 5), vec!["the quick brown", "fox jumps over", "the lazy dog"]);
        assert_eq!(wrap("the quick brown fox jumps over the lazy dog", 15, 2), vec!["the quick brown", "fox jumps over…"]);
        assert_eq!(wrap("a\n\nb\n", 20, 5), vec!["a", "", "b"]);
        let now = SystemTime::now();
        assert_eq!(ago(now, now), "now");
        assert_eq!(ago(now - Duration::from_secs(300), now), "5 min");
        assert_eq!(ago(now - Duration::from_secs(7300), now), "2 h");
    }
}
