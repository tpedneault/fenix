//! `SPC a U`, what the agents used: each agent's turns, time at work,
//! tokens and cost for today, this session and the last 7 days, a bar
//! for each of those days, and the threads of one of the three (`Tab`
//! for another). Enter shows the focused thread. `App`
//! (`app/agents/usage.rs`) gives it the ledger and keeps it current.

use fenix_acp::usage::{cost_text, duration_text, tokens_text, Budget, Day, Ledger, Tally};

use crate::page::{fit, frame, Grid, Key, Page, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Today,
    Session,
    Week,
}

impl Period {
    fn label(self) -> &'static str {
        match self {
            Period::Today => "Today",
            Period::Session => "This session",
            Period::Week => "Last 7 days",
        }
    }

    fn next(self) -> Self {
        match self {
            Period::Today => Period::Session,
            Period::Session => Period::Week,
            Period::Week => Period::Today,
        }
    }
}

pub struct UsagePage {
    pub ledger: Ledger,
    /// The last 7 days' dates, oldest first: the last is today.
    pub week: Vec<String>,
    pub budget: Option<Budget>,
    /// Whose threads are listed.
    pub period: Period,
    /// The focused thread's index in `threads()`.
    pub focus: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Show this thread.
    Open(String),
}

impl UsagePage {
    pub fn new(ledger: Ledger, week: Vec<String>, budget: Option<Budget>) -> Self {
        UsagePage { ledger, week, budget, period: Period::Today, focus: 0 }
    }

    pub fn today(&self) -> &str {
        self.week.last().map_or("", String::as_str)
    }

    fn day(&self, period: Period) -> Day {
        match period {
            Period::Today => self.ledger.over(&[self.today()]),
            Period::Session => self.ledger.session.clone(),
            Period::Week => self.ledger.over(&self.week),
        }
    }

    /// The period's threads, the most used first.
    pub fn threads(&self) -> Vec<(String, fenix_acp::usage::ThreadTally)> {
        let mut threads: Vec<_> = self.day(self.period).threads.into_iter().collect();
        threads.sort_by(|(_, a), (_, b)| weight(&b.tally).partial_cmp(&weight(&a.tally)).unwrap_or(std::cmp::Ordering::Equal));
        threads
    }

    /// Takes the newest ledger, keeping the focus on the thread it was on.
    pub fn update(&mut self, ledger: Ledger, week: Vec<String>, budget: Option<Budget>) {
        let focused = self.threads().get(self.focus).map(|(id, _)| id.clone());
        self.ledger = ledger;
        self.week = week;
        self.budget = budget;
        let threads = self.threads();
        self.focus = focused.and_then(|id| threads.iter().position(|(t, _)| *t == id)).unwrap_or(self.focus).min(threads.len().saturating_sub(1));
    }

    pub fn key(&mut self, key: Key) -> Action {
        let last = self.threads().len().saturating_sub(1);
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('j') | Key::Down => self.focus = (self.focus + 1).min(last),
            Key::Char('k') | Key::Up => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = last,
            Key::Tab => {
                self.period = self.period.next();
                self.focus = 0;
            }
            Key::Enter => return self.threads().get(self.focus).map_or(Action::None, |(id, _)| Action::Open(id.clone())),
            _ => {}
        }
        Action::None
    }
}

/// What sorts the threads: cost, then tokens, then time.
fn weight(t: &Tally) -> (f64, u64, u64) {
    (t.cost, t.tokens(), t.active_ms)
}

/// "2 turns · 3m 12s".
fn turns_and_time(t: &Tally) -> String {
    format!("{} turn{} · {}", t.turns, if t.turns == 1 { "" } else { "s" }, duration_text(t.active_ms))
}

fn cost_of(t: &Tally) -> String {
    cost_text(t.cost, t.currency.as_deref().unwrap_or("USD"))
}

/// An agent's session on the modeline, as `agents.usage.modeline` says
/// (`tokens`, `cost` or `all`): what it reported, else its turns and time.
pub fn brief(t: &Tally, mode: &str) -> String {
    let tokens = format!("{} tok", tokens_text(t.tokens()));
    match mode {
        "cost" if t.has_cost() => cost_of(t),
        "cost" | "tokens" if t.has_tokens() => tokens,
        "all" => {
            let mut parts = Vec::new();
            if t.has_tokens() {
                parts.push(tokens);
            }
            if t.has_cost() {
                parts.push(cost_of(t));
            }
            parts.push(turns_and_time(t));
            parts.join(" · ")
        }
        _ => turns_and_time(t),
    }
}

/// The columns after the name: turns, time, tokens, cache read, cost.
const COLUMNS: [(&str, usize); 5] = [("turns", 6), ("time", 9), ("tokens", 9), ("cache read", 12), ("cost", 10)];
const FIGURES: usize = 6 + 9 + 9 + 12 + 10;

fn figures(t: &Tally) -> String {
    let dash = || "—".to_string();
    let cells = [
        t.turns.to_string(),
        if t.turns > 0 { duration_text(t.active_ms) } else { dash() },
        if t.has_tokens() { tokens_text(t.tokens()) } else { dash() },
        if t.has_tokens() { tokens_text(t.cache_read) } else { dash() },
        if t.has_cost() { cost_of(t) } else { dash() },
    ];
    cells.iter().zip(COLUMNS).map(|(cell, (_, w))| format!("{cell:>w$}")).collect()
}

/// The day's date, shorter: `Fri 06`.
fn day_label(date: &str) -> String {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map(|d| d.format("%a %d").to_string()).unwrap_or_else(|_| date.to_string())
}

pub fn layout(page: &UsagePage, cols: usize) -> Page {
    let (left, width) = frame(cols, 100);
    let mut g = Grid::new();
    g.put(1, left, "Agent usage", Role::Title);
    let today = page.day(Period::Today).total();
    let mut said = format!("· {}", page.today());
    let mut role = Role::Muted;
    if let Some(budget) = &page.budget {
        let share = budget.share(&today);
        said.push_str(&format!(" · today's budget {}: {}% used", budget.describe(), (share * 100.0).round() as u64));
        role = if share >= 1.0 {
            Role::Bad
        } else if share >= 0.8 {
            Role::Warn
        } else {
            Role::Muted
        };
    }
    g.put(1, left + 12, &said, role);
    g.rule(2, left..left + width);
    let name_w = width.saturating_sub(FIGURES).max(10);
    let mut row = 4;
    let header = |g: &mut Grid, row: usize, title: &str| {
        g.put(row, left, title, Role::Title);
        let heads: String = COLUMNS.iter().map(|(h, w)| format!("{h:>w$}")).collect();
        g.put(row, left + name_w, &heads, Role::Muted);
    };
    let week = page.day(Period::Week);
    if week.agents.is_empty() && page.ledger.session.agents.is_empty() {
        g.put(row, left, "Nothing yet: what the agents use shows here once a turn ends.", Role::Muted);
        g.keys(left, width, &[("q", "close")]);
        return g.finish();
    }
    for period in [Period::Today, Period::Session, Period::Week] {
        header(&mut g, row, period.label());
        row += 1;
        let day = page.day(period);
        if day.agents.is_empty() {
            g.put(row, left + 2, "nothing", Role::Muted);
            row += 1;
        }
        for (agent, tally) in &day.agents {
            g.put(row, left + 2, &fit(agent, name_w.saturating_sub(3)), Role::Accent);
            g.put(row, left + name_w, &figures(tally), Role::Text);
            row += 1;
        }
        if day.agents.len() > 1 {
            g.put(row, left + 2, "all", Role::Muted);
            g.put(row, left + name_w, &figures(&day.total()), Role::Muted);
            row += 1;
        }
        row += 1;
    }

    // A bar a day: by cost when an agent said one, else tokens, else time.
    let days: Vec<(String, Tally)> = page.week.iter().map(|d| (d.clone(), page.ledger.over(&[d]).total())).collect();
    let (by, value): (&str, fn(&Tally) -> f64) = if days.iter().any(|(_, t)| t.has_cost()) {
        ("cost", |t| t.cost)
    } else if days.iter().any(|(_, t)| t.has_tokens()) {
        ("tokens", |t| t.tokens() as f64)
    } else {
        ("time at work", |t| t.active_ms as f64)
    };
    g.put(row, left, "By day", Role::Title);
    g.put(row, left + 7, &format!("· {by}"), Role::Muted);
    row += 1;
    let most = days.iter().map(|(_, t)| value(t)).fold(0.0, f64::max);
    let bar_w = width.saturating_sub(10 + 24).clamp(4, 40);
    for (date, t) in &days {
        let n = if most > 0.0 { (value(t) / most * bar_w as f64).round() as usize } else { 0 };
        let n = if n == 0 && value(t) > 0.0 { 1 } else { n };
        let is_today = date == page.today();
        g.put(row, left + 2, &day_label(date), if is_today { Role::Accent } else { Role::Muted });
        g.put(row, left + 10, &"█".repeat(n), if is_today { Role::Accent } else { Role::Text });
        let figure = match by {
            "cost" => cost_of(t),
            "tokens" => format!("{} tok", tokens_text(t.tokens())),
            _ => duration_text(t.active_ms),
        };
        if !t.is_empty() {
            g.put(row, left + 11 + bar_w, &figure, Role::Muted);
        }
        row += 1;
    }
    row += 1;

    g.put(row, left, &format!("Threads · {}", page.period.label().to_lowercase()), Role::Title);
    g.put(row, left + name_w, &COLUMNS.iter().map(|(h, w)| format!("{h:>w$}")).collect::<String>(), Role::Muted);
    row += 1;
    let threads = page.threads();
    if threads.is_empty() {
        g.put(row, left + 2, "none", Role::Muted);
    }
    let agent_w = 10.min(name_w / 3);
    for (i, (_, t)) in threads.iter().enumerate() {
        let title = if t.title.is_empty() { "(untitled)" } else { &t.title };
        g.put(row, left + 2, &fit(title, name_w.saturating_sub(agent_w + 4)), Role::Text);
        g.put(row, left + name_w.saturating_sub(agent_w + 1), &fit(&t.agent, agent_w), Role::Muted);
        g.put(row, left + name_w, &figures(&t.tally), Role::Text);
        if i == page.focus {
            g.focus(row, left..left + width);
        }
        row += 1;
    }
    g.keys(left, width, &[("↵", "show the thread"), ("Tab", "today / session / 7 days"), ("j/k", "move"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_acp::protocol::TurnUsage;

    fn tokens(input: u64, output: u64) -> Tally {
        Tally::of_tokens(&TurnUsage { input, output, cache_read: 40_000, ..TurnUsage::default() })
    }

    fn week() -> Vec<String> {
        (0..7).map(|d| format!("2026-11-{:02}", d + 1)).collect()
    }

    fn ledger() -> Ledger {
        let mut l = Ledger::default();
        l.days.entry("2026-11-03".into()).or_default().add("old", "claude", "Last week's", &tokens(5000, 1000));
        l.days.entry("2026-11-03".into()).or_default().add("old", "claude", "", &Tally::of_cost(2.0, "USD"));
        for spent in [tokens(10_000, 2400), Tally::of_cost(0.31, "USD"), Tally::of_turn(192_000)] {
            l.record("2026-11-07", "t1", "claude", "Fix the parser", &spent);
        }
        l.record("2026-11-07", "t2", "codex", "Write the tests", &Tally::of_turn(45_000));
        l
    }

    #[test]
    fn the_modeline_says_tokens_cost_or_turns_as_the_setting_and_the_agent_say() {
        let mut t = tokens(10_000, 2400);
        t.add(&Tally::of_cost(0.314, "USD"));
        t.add(&Tally::of_turn(192_000));
        assert_eq!(brief(&t, "tokens"), "12.4k tok");
        assert_eq!(brief(&t, "cost"), "$0.31");
        assert_eq!(brief(&t, "all"), "12.4k tok · $0.31 · 1 turn · 3m 12s");
        let codex = { let mut c = tokens(900, 100); c.add(&Tally::of_turn(5000)); c };
        assert_eq!(brief(&codex, "cost"), "1k tok", "no cost said: its tokens");
        let quiet = Tally { turns: 2, active_ms: 45_000, ..Tally::default() };
        for mode in ["tokens", "cost"] {
            assert_eq!(brief(&quiet, mode), "2 turns · 45s", "nothing said: turns and time");
        }
        assert_eq!(brief(&quiet, "all"), "2 turns · 45s");
    }

    #[test]
    fn it_lays_out_the_agents_by_period_a_bar_a_day_and_the_threads() {
        let mut p = UsagePage::new(ledger(), week(), Budget::parse("$1"));
        let text = layout(&p, 110).all_text();
        assert!(text.contains("Agent usage") && text.contains("2026-11-07"), "{text}");
        assert!(text.contains("today's budget $1.00: 31% used"), "{text}");
        for section in ["Today", "This session", "Last 7 days", "By day · cost", "Threads · today"] {
            assert!(text.contains(section), "{section}: {text}");
        }
        let line = |needle: &str| text.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("{needle}: {text}")).to_string();
        let claude = line("claude ");
        assert!(claude.contains("3m 12s") && claude.contains("12.4k") && claude.contains("40k") && claude.contains("$0.31"), "{claude}");
        let codex = line("codex ");
        assert!(codex.contains("45s") && codex.matches('—').count() == 3, "no tokens, cache or cost said: {codex}");
        // The 3rd's $2.00 is the longest bar; the 7th's is a sixth of it.
        let bar = |day: &str| line(day).matches('█').count();
        assert!(bar("Tue 03") > 5 * bar("Sat 07") && bar("Sat 07") > 0, "{text}");
        assert_eq!(bar("Sun 01"), 0);
        // Today's threads, the costliest first; Tab goes to the week's.
        assert!(text.find("Fix the parser").unwrap() < text.find("Write the tests").unwrap());
        assert!(!text.contains("Last week's"));
        assert_eq!(p.key(Key::Enter), Action::Open("t1".into()));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), Action::Open("t2".into()));
        p.key(Key::Tab);
        p.key(Key::Tab);
        assert_eq!(p.period, Period::Week);
        let text = layout(&p, 110).all_text();
        assert!(text.contains("Threads · last 7 days") && text.contains("Last week's"), "{text}");
        assert_eq!(p.key(Key::Enter), Action::Open("old".into()), "the costliest");
        assert_eq!(p.key(Key::Char('q')), Action::Close);
    }

    #[test]
    fn with_nothing_used_it_says_so() {
        let text = layout(&UsagePage::new(Ledger::default(), week(), None), 100).all_text();
        assert!(text.contains("Nothing yet"), "{text}");
    }

    #[test]
    fn without_cost_the_bars_are_tokens_then_time() {
        let mut l = Ledger::default();
        l.record("2026-11-07", "t", "codex", "T", &tokens(1000, 0));
        assert!(layout(&UsagePage::new(l, week(), None), 100).all_text().contains("By day · tokens"));
        let mut l = Ledger::default();
        l.record("2026-11-07", "t", "gemini", "T", &Tally::of_turn(1000));
        assert!(layout(&UsagePage::new(l, week(), None), 100).all_text().contains("By day · time at work"));
    }
}
