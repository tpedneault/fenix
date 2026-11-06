//! What the agents used: turns, time at work, tokens and cost, per agent
//! and per thread, for this session and for each day. Pure -- the
//! caller says which day it is and how long a turn took; the GUI keeps
//! the days in `agent-usage.json`, beside the threads.
//!
//! What's reported comes from two places. A turn's tokens are in
//! `session/prompt`'s answer (`TurnUsage`), for that turn alone. Cost is
//! in `usage_update`, the session's running total (Claude Code's adapter
//! sends it; Codex's doesn't), so the hub hands on what it went up by.
//! An agent that reports neither still has its turns and time counted.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::protocol::TurnUsage;

/// Days kept on disk.
pub const KEEP_DAYS: usize = 90;

/// Totals for an agent, a thread, or several.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tally {
    pub turns: u64,
    /// Time at work: each turn's start to its end.
    pub active_ms: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Turns that said what tokens they used.
    pub reports: u64,
    /// What it cost, in `currency`, when the agent says.
    pub cost: f64,
    pub currency: Option<String>,
}

impl Tally {
    /// A turn's tokens.
    pub fn of_tokens(usage: &TurnUsage) -> Self {
        Tally { input: usage.input, output: usage.output, cache_read: usage.cache_read, cache_write: usage.cache_write, reports: 1, ..Tally::default() }
    }

    pub fn of_cost(amount: f64, currency: &str) -> Self {
        Tally { cost: amount, currency: Some(currency.to_string()), ..Tally::default() }
    }

    /// A turn done, after `active_ms`.
    pub fn of_turn(active_ms: u64) -> Self {
        Tally { turns: 1, active_ms, ..Tally::default() }
    }

    /// The tokens the model took in and wrote: input, output and what
    /// went into the cache. What it read back from the cache -- the same
    /// context again, every request -- is `cache_read`, apart.
    pub fn tokens(&self) -> u64 {
        self.input + self.output + self.cache_write
    }

    pub fn has_tokens(&self) -> bool {
        self.reports > 0
    }

    pub fn has_cost(&self) -> bool {
        self.currency.is_some()
    }

    /// Another's, added. Costs in another currency are added as if they
    /// were this one's: agents all say USD.
    pub fn add(&mut self, other: &Tally) {
        self.turns += other.turns;
        self.active_ms += other.active_ms;
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.reports += other.reports;
        self.cost += other.cost;
        if self.currency.is_none() {
            self.currency.clone_from(&other.currency);
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Tally::default()
    }
}

/// A thread's totals, with what it was called (it may be gone since).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadTally {
    pub agent: String,
    pub title: String,
    #[serde(flatten)]
    pub tally: Tally,
}

/// A day's (or a session's) totals, by agent id and by thread id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Day {
    pub agents: BTreeMap<String, Tally>,
    pub threads: BTreeMap<String, ThreadTally>,
}

impl Day {
    pub fn add(&mut self, thread: &str, agent: &str, title: &str, spent: &Tally) {
        self.agents.entry(agent.to_string()).or_default().add(spent);
        let t = self.threads.entry(thread.to_string()).or_default();
        t.agent = agent.to_string();
        if !title.is_empty() {
            t.title = title.to_string();
        }
        t.tally.add(spent);
    }

    pub fn merge(&mut self, other: &Day) {
        for (agent, tally) in &other.agents {
            self.agents.entry(agent.clone()).or_default().add(tally);
        }
        for (id, t) in &other.threads {
            let mine = self.threads.entry(id.clone()).or_default();
            mine.agent.clone_from(&t.agent);
            if !t.title.is_empty() {
                mine.title.clone_from(&t.title);
            }
            mine.tally.add(&t.tally);
        }
    }

    /// Every agent's, together.
    pub fn total(&self) -> Tally {
        let mut total = Tally::default();
        for tally in self.agents.values() {
            total.add(tally);
        }
        total
    }
}

/// The days, and this session.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ledger {
    /// By date, `YYYY-MM-DD`.
    pub days: BTreeMap<String, Day>,
    /// Since Fenix started: not kept.
    pub session: Day,
    /// The budget warning given last: its day and level (80, 100).
    pub warned: Option<(String, u8)>,
}

impl Ledger {
    /// What thread `thread` of agent `agent` spent, on `day`.
    pub fn record(&mut self, day: &str, thread: &str, agent: &str, title: &str, spent: &Tally) {
        self.session.add(thread, agent, title, spent);
        self.days.entry(day.to_string()).or_default().add(thread, agent, title, spent);
        while self.days.len() > KEEP_DAYS {
            self.days.pop_first();
        }
    }

    /// The totals of these days.
    pub fn over<S: AsRef<str>>(&self, days: &[S]) -> Day {
        let mut sum = Day::default();
        for day in days {
            if let Some(d) = self.days.get(day.as_ref()) {
                sum.merge(d);
            }
        }
        sum
    }

    /// The level of `budget` that today's use has crossed and that
    /// wasn't said yet today: 80 (percent), then 100. Each once a day.
    pub fn budget_crossed(&mut self, day: &str, budget: &Budget) -> Option<u8> {
        let share = budget.share(&self.days.get(day).map(Day::total).unwrap_or_default());
        let level = if share >= 1.0 {
            100
        } else if share >= 0.8 {
            80
        } else {
            return None;
        };
        let said = self.warned.as_ref().filter(|(d, _)| d == day).map_or(0, |(_, l)| *l);
        if level <= said {
            return None;
        }
        self.warned = Some((day.to_string(), level));
        Some(level)
    }

    pub fn to_json(&self) -> Value {
        json!({
            "version": 1,
            "days": self.days,
            "warned": self.warned.as_ref().map(|(day, level)| json!({ "day": day, "level": level })),
        })
    }

    /// Lenient: a day that doesn't read is skipped, not the file.
    pub fn from_json(v: &Value) -> Self {
        let days = v
            .get("days")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, d)| Some((k.clone(), serde_json::from_value(d.clone()).ok()?))).collect())
            .unwrap_or_default();
        let warned = v.get("warned").and_then(|w| Some((w.get("day")?.as_str()?.to_string(), w.get("level")?.as_u64()? as u8)));
        Ledger { days, session: Day::default(), warned }
    }

    /// What `path` holds; nothing when it's missing or unreadable.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| Self::from_json(&v)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        fenix_storage::write(path, &serde_json::to_vec(&self.to_json())?)
    }
}

/// `agents.usage.daily_budget`: a day's cost (`5`, `$5`, `5 USD`) or
/// tokens (`2M`, `500k`, `300000 tokens`).
#[derive(Clone, Debug, PartialEq)]
pub enum Budget {
    Cost(f64),
    Tokens(u64),
}

impl Budget {
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim().to_lowercase();
        if t.is_empty() {
            return None;
        }
        let (t, tokens) = match t.strip_suffix("tokens").or_else(|| t.strip_suffix("tok")) {
            Some(rest) => (rest.trim_end().to_string(), true),
            None => (t.clone(), false),
        };
        let (number, scale) = match t.chars().last()? {
            'k' => (&t[..t.len() - 1], Some(1e3)),
            'm' => (&t[..t.len() - 1], Some(1e6)),
            _ => (t.as_str(), None),
        };
        if tokens || scale.is_some() {
            let n: f64 = number.trim().parse().ok()?;
            return (n > 0.0).then(|| Budget::Tokens((n * scale.unwrap_or(1.0)).round() as u64));
        }
        let number = number.trim().trim_start_matches('$').trim_end_matches("usd").trim();
        let n: f64 = number.parse().ok()?;
        (n > 0.0 && n.is_finite()).then_some(Budget::Cost(n))
    }

    /// How much of it `used` is: 1.0 is all of it. A cost budget of an
    /// agent that says no cost is never reached.
    pub fn share(&self, used: &Tally) -> f64 {
        match self {
            Budget::Cost(limit) => used.cost / limit,
            Budget::Tokens(limit) => used.tokens() as f64 / *limit as f64,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Budget::Cost(limit) => cost_text(*limit, "USD"),
            Budget::Tokens(limit) => format!("{} tokens", tokens_text(*limit)),
        }
    }
}

/// `950`, `12.4k`, `3.1M`.
pub fn tokens_text(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => trim_point(format!("{:.1}", n as f64 / 1e3)) + "k",
        _ => trim_point(format!("{:.1}", n as f64 / 1e6)) + "M",
    }
}

fn trim_point(s: String) -> String {
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

/// `$0.31`, or `0.31 EUR`.
pub fn cost_text(amount: f64, currency: &str) -> String {
    let figure = if amount != 0.0 && amount.abs() < 0.01 { format!("{amount:.3}") } else { format!("{amount:.2}") };
    if currency.eq_ignore_ascii_case("USD") {
        format!("${figure}")
    } else {
        format!("{figure} {currency}")
    }
}

/// `45s`, `3m 12s`, `1h 05m`.
pub fn duration_text(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, secs / 60 % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Tally {
        Tally::of_tokens(&TurnUsage { input, output, cache_read, cache_write, thought: 0 })
    }

    #[test]
    fn a_thread_and_its_agent_add_up_for_the_day_and_the_session() {
        let mut l = Ledger::default();
        l.record("2026-11-05", "t1", "claude", "Fix the parser", &tokens(1000, 200, 50_000, 300));
        l.record("2026-11-06", "t1", "claude", "", &Tally::of_cost(0.25, "USD"));
        l.record("2026-11-06", "t1", "claude", "", &Tally::of_turn(65_000));
        l.record("2026-11-06", "t2", "codex", "Tests", &tokens(10, 20, 0, 0));
        let claude = &l.session.agents["claude"];
        assert_eq!((claude.turns, claude.tokens(), claude.cache_read, claude.cost), (1, 1500, 50_000, 0.25));
        assert_eq!(l.session.threads["t1"].title, "Fix the parser", "an empty title leaves the one it had");
        assert_eq!(l.days["2026-11-06"].agents["claude"].tokens(), 0, "the tokens were the day before");
        let week = l.over(&["2026-11-05", "2026-11-06", "2026-11-07"]);
        assert_eq!(week.agents["claude"], *claude);
        assert_eq!(week.total().tokens(), 1530);
        assert!(!week.agents["codex"].has_cost() && week.agents["codex"].has_tokens());
    }

    #[test]
    fn the_days_go_to_disk_and_come_back_without_the_session() {
        let mut l = Ledger::default();
        l.record("2026-11-06", "t1", "claude", "A", &tokens(1, 2, 3, 4));
        l.record("2026-11-06", "t1", "claude", "A", &Tally::of_cost(1.5, "USD"));
        l.warned = Some(("2026-11-06".into(), 80));
        let back = Ledger::from_json(&l.to_json());
        assert_eq!(back.days, l.days);
        assert_eq!(back.warned, l.warned);
        assert!(back.session.agents.is_empty());
        // A day an older or newer Fenix wrote oddly is skipped, not the file.
        let odd = json!({ "days": { "2026-11-01": "what", "2026-11-02": { "agents": { "claude": { "turns": 3, "new_field": true } } } } });
        let back = Ledger::from_json(&odd);
        assert_eq!(back.days.len(), 1);
        assert_eq!(back.days["2026-11-02"].agents["claude"].turns, 3);
    }

    #[test]
    fn only_the_newest_days_are_kept() {
        let mut l = Ledger::default();
        for d in 0..(KEEP_DAYS + 5) {
            l.record(&format!("2026-{:02}-{:02}", 1 + d / 28, 1 + d % 28), "t", "a", "", &Tally::of_turn(1));
        }
        assert_eq!(l.days.len(), KEEP_DAYS);
        assert!(!l.days.contains_key("2026-01-01"));
    }

    #[test]
    fn a_budget_warns_at_80_and_100_percent_once_a_day() {
        let budget = Budget::parse("$2").unwrap();
        let mut l = Ledger::default();
        l.record("d1", "t", "claude", "", &Tally::of_cost(1.0, "USD"));
        assert_eq!(l.budget_crossed("d1", &budget), None);
        l.record("d1", "t", "claude", "", &Tally::of_cost(0.7, "USD"));
        assert_eq!(l.budget_crossed("d1", &budget), Some(80));
        assert_eq!(l.budget_crossed("d1", &budget), None, "said once");
        l.record("d1", "t", "claude", "", &Tally::of_cost(0.5, "USD"));
        assert_eq!(l.budget_crossed("d1", &budget), Some(100));
        assert_eq!(l.budget_crossed("d1", &budget), None);
        l.record("d2", "t", "claude", "", &Tally::of_cost(2.0, "USD"));
        assert_eq!(l.budget_crossed("d2", &budget), Some(100), "a new day");
        let tokens = Budget::parse("1k").unwrap();
        l.record("d3", "t", "codex", "", &tokens_of(900));
        assert_eq!(l.budget_crossed("d3", &tokens), Some(80));
    }

    fn tokens_of(n: u64) -> Tally {
        tokens(0, n, 0, 0)
    }

    #[test]
    fn budgets_read_as_cost_or_tokens() {
        assert_eq!(Budget::parse("5"), Some(Budget::Cost(5.0)));
        assert_eq!(Budget::parse(" $2.50 "), Some(Budget::Cost(2.5)));
        assert_eq!(Budget::parse("3 USD"), Some(Budget::Cost(3.0)));
        assert_eq!(Budget::parse("2M"), Some(Budget::Tokens(2_000_000)));
        assert_eq!(Budget::parse("500k"), Some(Budget::Tokens(500_000)));
        assert_eq!(Budget::parse("1.5m tokens"), Some(Budget::Tokens(1_500_000)));
        assert_eq!(Budget::parse("300000 tokens"), Some(Budget::Tokens(300_000)));
        assert_eq!(Budget::parse("300000 tok"), Some(Budget::Tokens(300_000)));
        for nothing in ["", "lots", "-1", "0", "$"] {
            assert_eq!(Budget::parse(nothing), None, "{nothing}");
        }
        assert_eq!(Budget::Tokens(2_000_000).describe(), "2M tokens");
        assert_eq!(Budget::Cost(5.0).describe(), "$5.00");
    }

    #[test]
    fn figures_are_short() {
        assert_eq!(tokens_text(950), "950");
        assert_eq!(tokens_text(12_400), "12.4k");
        assert_eq!(tokens_text(12_000), "12k");
        assert_eq!(tokens_text(3_140_000), "3.1M");
        assert_eq!(cost_text(0.314, "USD"), "$0.31");
        assert_eq!(cost_text(0.004, "USD"), "$0.004");
        assert_eq!(cost_text(2.0, "EUR"), "2.00 EUR");
        assert_eq!(duration_text(45_900), "45s");
        assert_eq!(duration_text(192_000), "3m 12s");
        assert_eq!(duration_text(3_900_000), "1h 05m");
    }
}
