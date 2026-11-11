//! A quiet line on Home when a newer Fenix is out: GitHub's latest
//! release, looked for at most once a day while `updates.check` is on
//! (it's off unless you turn it on), or when asked
//! (`app.check_for_updates`). A machine's `policy.toml` can say no for
//! everyone (`update_check = false`). Nothing is downloaded or installed.
//! Nightlies are told about only with `updates.channel = "nightly"`; a
//! nightly build says what it is on Home whatever the channel.

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use fenix_extensions::update::{self, Channel, Newer, Seen};

use super::{App, FenixUserEvent};

/// What the update check knows.
#[derive(Debug, Default)]
pub(crate) struct UpdateState {
    /// Where it keeps what it found (`updates.json`).
    pub(crate) file: Option<PathBuf>,
    /// What it found when it last looked.
    pub(crate) seen: Option<Seen>,
    /// A look under way, and whether it was asked for (then what it finds
    /// is said, whatever it is).
    pub(crate) looking: Option<bool>,
    /// The machine's policy, in place of `policy.toml`'s, for tests.
    #[cfg(test)]
    pub(crate) policy: Option<fenix_extensions::Policy>,
}

/// A look's answer, from its thread.
#[derive(Debug)]
pub(crate) struct UpdateAnswer {
    pub(crate) asked: bool,
    pub(crate) result: Result<Seen, String>,
}

impl App {
    /// Whether this machine's policy lets anyone here look.
    fn updates_allowed(&self) -> Result<(), String> {
        #[cfg(test)]
        let policy = self.updates.policy.clone().unwrap_or_default();
        #[cfg(not(test))]
        let policy = crate::installed::machine().map(|m| m.policy()).transpose()?.unwrap_or_default();
        if policy.update_check == Some(false) {
            return Err("looking for a newer Fenix is switched off on this machine by its administrator".into());
        }
        Ok(())
    }

    /// Which builds are told about (`updates.channel`).
    fn update_channel(&self) -> Channel {
        Channel::from_setting(self.config.text_value("updates.channel"))
    }

    /// What was found, read from its file the first time.
    fn update_seen(&mut self) -> &Seen {
        if self.updates.seen.is_none() {
            let seen = self.updates.file.as_deref().map(Seen::read).unwrap_or_default();
            self.updates.seen = Some(seen);
        }
        self.updates.seen.get_or_insert_with(Seen::default)
    }

    /// Home's line: a newer Fenix, when one was found.
    pub(crate) fn update_notice(&mut self) -> Option<String> {
        if self.updates_allowed().is_err() {
            return None;
        }
        let channel = self.update_channel();
        match self.update_seen().newer_than(&crate::installed::release(), crate::installed::COMMIT, channel)? {
            Newer::Release(v) => Some(format!("Fenix {v} is out -- {}", update::RELEASES_PAGE.trim_start_matches("https://"))),
            Newer::Nightly { version, commit } => {
                let what = crate::installed::describe_with(&version.to_string(), commit.as_deref());
                Some(format!("Fenix {what} (nightly) is out -- {}", update::NIGHTLY_PAGE.trim_start_matches("https://")))
            }
        }
    }

    /// Home's line under the date: a newer Fenix, or, on a nightly build
    /// with nothing newer, which nightly this is.
    pub(crate) fn home_update_line(&mut self) -> Option<String> {
        self.update_notice().or_else(|| crate::installed::is_nightly().then(|| format!("Nightly {}", crate::installed::describe())))
    }

    /// `app.check_for_updates`: looks now, and says what it finds.
    pub(crate) fn cmd_check_for_updates(&mut self) {
        if let Err(why) = self.updates_allowed() {
            self.set_error(why);
            return;
        }
        // The daily look under way already: what it finds is said, as if
        // asked.
        if let Some(asked) = self.updates.looking.as_mut() {
            *asked = true;
            self.set_message("looking for a newer Fenix…");
            return;
        }
        if self.start_update_check(true) {
            self.set_message("looking for a newer Fenix…");
        }
    }

    /// A look on another thread: false when one's under way already, or
    /// there's no event loop to hear back on (tests).
    fn start_update_check(&mut self, asked: bool) -> bool {
        if self.updates.looking.is_some() {
            return false;
        }
        let Some(proxy) = self.event_proxy.clone() else { return false };
        self.updates.looking = Some(asked);
        let channel = self.update_channel();
        let spawned = std::thread::Builder::new().name("fenix-updates".into()).spawn(move || {
            let result = update::check(&fenix_extensions::registry::Http, SystemTime::now(), channel);
            let _ = proxy.send_event(FenixUserEvent::Updates(UpdateAnswer { asked, result }));
        });
        if spawned.is_err() {
            self.updates.looking = None;
            return false;
        }
        true
    }

    /// A look's answer: kept, shown on Home, and said when it was asked.
    pub(crate) fn apply_update_answer(&mut self, answer: UpdateAnswer) {
        // Asked for while it was under way counts as asked.
        let asked = answer.asked || self.updates.looking == Some(true);
        let answer = UpdateAnswer { asked, ..answer };
        self.updates.looking = None;
        match answer.result {
            Ok(seen) => {
                if let Some(file) = &self.updates.file {
                    if let Err(e) = seen.write(file) {
                        self.set_error(format!("couldn't keep what the update check found: {e}"));
                    }
                }
                self.updates.seen = Some(seen);
                if answer.asked {
                    match self.update_notice() {
                        Some(notice) => self.set_message(notice),
                        None => self.set_message(format!("Fenix {} is the newest", crate::installed::describe())),
                    }
                }
                self.refresh_home_data(true);
            }
            Err(why) => {
                // The daily look says nothing when it can't: it's offline
                // as often as not. When it was asked, it says why.
                if answer.asked {
                    self.set_error(format!("couldn't look for a newer Fenix: {why}"));
                }
                // Not looked again for a day either way -- kept, so a
                // machine that's offline doesn't ask at every start.
                let checked = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let seen = Seen { checked, ..self.update_seen().clone() };
                if let Some(file) = &self.updates.file {
                    let _ = seen.write(file);
                }
                self.updates.seen = Some(seen);
            }
        }
    }

    /// The daily look, when `updates.check` is on and it's due: when to
    /// look again.
    pub(crate) fn update_tick(&mut self, now: Instant) -> Option<Instant> {
        if self.config.bool_value("updates.check") != Some(true) || self.updates.looking.is_some() {
            return None;
        }
        let wall = SystemTime::now();
        let next = self.update_seen().next(wall);
        if next > wall {
            return Some(now + next.duration_since(wall).unwrap_or_default());
        }
        // Due. The machine's policy is read only now, not on every wake.
        if self.updates_allowed().is_ok() && self.start_update_check(false) {
            return None;
        }
        // Not allowed, or nothing to look with: not asked again for a day.
        let checked = wall.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        self.updates.seen = Some(Seen { checked, ..self.update_seen().clone() });
        Some(now + update::EVERY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(seen: Seen) -> App {
        let mut app = App::with_file(None);
        app.updates.seen = Some(seen);
        app
    }

    #[test]
    fn home_says_so_only_when_there_s_a_newer_release() {
        let mut app = app_with(Seen { checked: 1, latest: Some("v99.0.0".into()), nightly: None });
        let notice = app.update_notice().expect("a newer one");
        assert!(notice.starts_with("Fenix 99.0.0 is out"), "{notice}");
        let mut app = app_with(Seen { checked: 1, latest: Some(format!("v{}", crate::installed::release())), nightly: None });
        assert_eq!(app.update_notice(), None, "the one running");
        let mut app = app_with(Seen::default());
        assert_eq!(app.update_notice(), None, "never looked");
    }

    #[test]
    fn a_nightly_is_on_home_only_on_the_nightly_channel() {
        let nightly = update::Nightly { version: "99.0.0-nightly.20991231".into(), commit: Some("0123456789abcdef0123456789abcdef01234567".into()) };
        let seen = Seen { checked: 1, latest: Some(format!("v{}", crate::installed::release())), nightly: Some(nightly) };
        let mut app = app_with(seen);
        assert_eq!(app.config.text_value("updates.channel"), None, "stable by default");
        assert_eq!(app.update_notice(), None, "releases alone");
        app.config.set_value("updates.channel", Some(fenix_config::Value::Text("nightly".into())));
        let notice = app.update_notice().expect("the nightly");
        assert!(notice.starts_with("Fenix 99.0.0-nightly.20991231 (0123456789ab) (nightly) is out -- github.com/tpedneault/fenix/releases/tag/nightly"), "{notice}");
        app.config.set_value("updates.channel", Some(fenix_config::Value::Text("stable".into())));
        assert_eq!(app.update_notice(), None, "back to releases alone at once");
    }

    #[test]
    fn home_s_line_is_the_notice_or_which_nightly_this_is() {
        let mut app = app_with(Seen { checked: 1, latest: Some("v99.0.0".into()), nightly: None });
        assert_eq!(app.home_update_line(), app.update_notice());
        let mut app = app_with(Seen::default());
        let line = app.home_update_line();
        if crate::installed::is_nightly() {
            assert_eq!(line, Some(format!("Nightly {}", crate::installed::describe())));
        } else {
            assert_eq!(line, None, "a release says nothing when nothing's newer");
        }
    }

    #[test]
    fn off_unless_turned_on_and_nothing_is_fetched_without_an_event_loop() {
        let mut app = app_with(Seen::default());
        assert_eq!(app.config.bool_value("updates.check"), None, "off by default");
        assert_eq!(app.update_tick(Instant::now()), None);
        assert!(app.updates.looking.is_none(), "not looking");
        app.config.set_value("updates.check", Some(fenix_config::Value::Bool(true)));
        let next = app.update_tick(Instant::now());
        assert!(app.updates.looking.is_none(), "no event loop: nothing fetched");
        assert!(next.is_some_and(|n| n > Instant::now() + std::time::Duration::from_secs(23 * 60 * 60)), "and not tried again at once");
        // Looked a moment ago: the next look is a day away.
        let checked = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        app.updates.seen = Some(Seen { checked, latest: None, nightly: None });
        let next = app.update_tick(Instant::now()).expect("a day away");
        assert!(next > Instant::now() + std::time::Duration::from_secs(23 * 60 * 60));
    }

    #[test]
    fn a_machine_s_policy_switches_it_off_for_everyone() {
        let mut app = app_with(Seen { checked: 1, latest: Some("v99.0.0".into()), nightly: None });
        app.config.set_value("updates.check", Some(fenix_config::Value::Bool(true)));
        app.updates.policy = Some(fenix_extensions::Policy { update_check: Some(false), ..Default::default() });
        assert_eq!(app.update_notice(), None, "nothing on Home");
        app.cmd_check_for_updates();
        assert!(app.modeline_text().contains("switched off on this machine"), "{}", app.modeline_text());
        app.updates.seen = Some(Seen::default());
        app.update_tick(Instant::now());
        assert!(app.updates.looking.is_none(), "no daily look");
    }

    #[test]
    fn asked_while_the_daily_look_is_under_way_its_answer_is_said() {
        let mut app = app_with(Seen::default());
        app.updates.looking = Some(false);
        app.cmd_check_for_updates();
        assert_eq!(app.updates.looking, Some(true));
        app.apply_update_answer(UpdateAnswer { asked: false, result: Ok(Seen { checked: 5, latest: Some(format!("v{}", crate::installed::release())), nightly: None }) });
        assert!(app.modeline_text().contains("is the newest"), "{}", app.modeline_text());
    }

    #[test]
    fn a_failed_look_is_kept_so_a_restart_doesn_t_ask_again() {
        let mut app = app_with(Seen::default());
        app.apply_update_answer(UpdateAnswer { asked: false, result: Err("offline".into()) });
        let file = app.updates.file.clone().unwrap();
        assert!(!Seen::read(&file).due(SystemTime::now()), "on disk too");
    }

    #[test]
    fn an_answer_is_kept_said_when_asked_and_a_failure_waits_a_day() {
        let mut app = app_with(Seen::default());
        app.apply_update_answer(UpdateAnswer { asked: true, result: Ok(Seen { checked: 5, latest: Some("v99.0.0".into()), nightly: None }) });
        assert!(app.modeline_text().contains("Fenix 99.0.0 is out"), "{}", app.modeline_text());
        assert!(app.home_data.update.as_deref().is_some_and(|u| u.contains("99.0.0")), "on Home");
        app.apply_update_answer(UpdateAnswer { asked: false, result: Err("offline".into()) });
        let seen = app.updates.seen.clone().unwrap();
        assert_eq!(seen.latest.as_deref(), Some("v99.0.0"), "what was found is kept");
        assert!(!seen.due(SystemTime::now()), "not looked again for a day");
    }
}
