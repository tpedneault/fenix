//! What the agents used (`fenix_acp::usage`): counted as turns end and
//! agents report, kept by day in `agent-usage.json` beside the threads,
//! said on the modeline (`agents.usage.modeline`), shown on `SPC a U`
//! (`agent_usage_page`), and measured against `agents.usage.daily_budget`.

use super::*;
use crate::agent_usage_page::{self, Action as UsageAction, UsagePage};
use fenix_acp::usage::{cost_text, tokens_text, Budget, Day, Ledger, Tally};

/// Today's date, as the ledger keys its days.
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The last 7 days, oldest first, today last.
fn last_week() -> Vec<String> {
    let today = chrono::Local::now().date_naive();
    (0..7).rev().map(|n| (today - chrono::Days::new(n)).format("%Y-%m-%d").to_string()).collect()
}

/// Where the days are kept: beside the threads' folder.
pub(super) fn usage_file() -> Option<PathBuf> {
    if cfg!(test) {
        Some(crate::app::isolated_test_path("agent-usage.json"))
    } else {
        fenix_storage::paths::state_file("agent-usage.json")
    }
}

pub(super) fn load_usage(file: Option<&Path>) -> Ledger {
    file.map(Ledger::load).unwrap_or_default()
}

impl App {
    fn agents_budget(&self) -> Option<Budget> {
        self.config.text_value("agents.usage.daily_budget").and_then(Budget::parse)
    }

    /// Thread `thread` spent `spent`: counted for today and the session,
    /// kept, and measured against the day's budget.
    pub(super) fn agents_used(&mut self, thread: &ThreadId, spent: Tally) {
        if spent.is_empty() {
            return;
        }
        let Some(view) = self.agents.hub.view(thread) else { return };
        let (agent, title) = (view.meta.agent.clone(), view.thread.display_title());
        let day = today();
        self.agents.usage.record(&day, &thread.0, &agent, &title, &spent);
        if let Some(budget) = self.agents_budget() {
            if let Some(level) = self.agents.usage.budget_crossed(&day, &budget) {
                let used = self.agents.usage.days.get(&day).map(Day::total).unwrap_or_default();
                let used = match budget {
                    Budget::Cost(_) => cost_text(used.cost, used.currency.as_deref().unwrap_or("USD")),
                    Budget::Tokens(_) => format!("{} tokens", tokens_text(used.tokens())),
                };
                let (summary, urgency) = if level >= 100 {
                    (format!("The agents reached today's budget: {used} of {}", budget.describe()), crate::notify::Urgency::Critical)
                } else {
                    (format!("The agents used {level}% of today's budget: {used} of {}", budget.describe()), crate::notify::Urgency::Normal)
                };
                self.notify(crate::notify::Request::new("agents", summary).body("SPC a U has what each agent and thread used.").urgency(urgency).action("Show the usage", "agents.usage"));
            }
        }
        if let Some(file) = &self.agents.usage_file {
            // Kept as it goes: a day's figures are worth less than a write.
            let _ = self.agents.usage.save(file);
        }
        self.refresh_agents_usage_page();
    }

    /// The modeline's usage segment: each agent used this session, as
    /// `agents.usage.modeline` says.
    pub(crate) fn agents_usage_segment(&self) -> String {
        let mode = self.config.text_value("agents.usage.modeline").unwrap_or("tokens");
        if mode == "off" {
            return String::new();
        }
        self.agents.usage.session.agents.iter().filter(|(_, t)| !t.is_empty()).map(|(agent, t)| format!("   ◆ {agent} {}", agent_usage_page::brief(t, mode))).collect()
    }

    /// `SPC a U`: the usage page.
    pub(crate) fn open_agents_usage_page(&mut self) {
        match self.find_page(|m| matches!(m, PageModel::AgentUsage(_))) {
            Some(id) => {
                self.refresh_agents_usage_page();
                self.show_page(id);
            }
            None => {
                let page = UsagePage::new(self.agents.usage.clone(), last_week(), self.agents_budget());
                self.open_page(PageModel::AgentUsage(Box::new(page)));
            }
        }
    }

    fn refresh_agents_usage_page(&mut self) {
        let Some(id) = self.find_page(|m| matches!(m, PageModel::AgentUsage(_))) else { return };
        let (ledger, budget) = (self.agents.usage.clone(), self.agents_budget());
        if let Some(state) = self.pages.get_mut(&id) {
            if let PageModel::AgentUsage(page) = &mut state.model {
                page.update(ledger, last_week(), budget);
            }
            state.stale = true;
        }
    }

    pub(crate) fn agents_usage_page_action(&mut self, id: BufferId, action: UsageAction) {
        match action {
            UsageAction::None => self.restale_page(id),
            UsageAction::Close => self.close_page(id),
            UsageAction::Open(thread) => {
                let thread = ThreadId(thread);
                if self.agents.hub.view(&thread).is_some() {
                    self.agents_show(thread);
                } else {
                    self.set_message("that thread was deleted");
                }
            }
        }
    }
}
