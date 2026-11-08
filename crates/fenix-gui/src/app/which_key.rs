//! The which-key menu: what the keys pressed so far can go on to. The
//! leader's (`SPC`) and the local leader's open as the palette
//! (`palette::layout`): the keys so far on caps in its input row, the
//! chords run lately, then each section's keys in columns -- groups in
//! their colour with how many commands they hold. Vim's own prefixes
//! (`g`, `z`, `[`, `]`, an operator waiting for its motion) show in the
//! hint bar growing out of the modeline (`palette::hint_bar`) instead:
//! they're about the code under the cursor, which a card over the
//! editor would hide. Either waits `which_key.delay_ms` before opening,
//! so a quick `SPC f f` or `dw` never flashes it; Backspace goes up a
//! level and `Ctrl-n`/`Ctrl-p` page through one too big for the room.

use super::*;
use fenix_keymap::Hint;

/// Whose keys the menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    Leader,
    Local,
    Vim,
}

/// Where the menu is, and what it shows.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Pending {
    /// The keys pressed so far, as keycaps.
    pub path: Vec<KeyPress>,
    pub title: String,
    pub hints: Vec<Hint>,
    pub source: Source,
}

/// The order keys are listed in: letters, then capitals, then the rest,
/// then named keys -- so a key keeps its place whatever it's called.
fn key_rank(key: &KeyPress) -> (u8, u8, char) {
    let mods = u8::from(key.mods.ctrl) + u8::from(key.mods.alt) * 2 + u8::from(key.mods.super_) * 4;
    match key.code {
        KeyCode::Char(c) if c.is_ascii_lowercase() => (0, mods, c),
        KeyCode::Char(c) if c.is_ascii_uppercase() => (1, mods, c.to_ascii_lowercase()),
        KeyCode::Char(' ') => (3, mods, ' '),
        KeyCode::Char(c) => (2, mods, c),
        KeyCode::Named(_) => (4, mods, ' '),
    }
}

/// `hints` in sections, each `(name, hints)`, in the order they're drawn.
pub(super) fn sections(mut hints: Vec<Hint>, by_label: bool) -> Vec<(String, Vec<Hint>)> {
    if by_label {
        hints.sort_by(|a, b| a.label.cmp(b.label));
    } else {
        hints.sort_by_key(|h| key_rank(&h.key));
    }
    let named: Vec<&'static str> = {
        let mut names: Vec<&'static str> = hints.iter().filter_map(|h| h.section).collect();
        names.sort_by_key(|n| keymap::WHICH_KEY_SECTIONS.iter().position(|s| s == n).unwrap_or(usize::MAX));
        names.dedup();
        names
    };
    let mut out: Vec<(String, Vec<Hint>)> = Vec::new();
    for name in &named {
        out.push((name.to_string(), hints.iter().filter(|h| h.section == Some(name)).copied().collect()));
    }
    let groups: Vec<Hint> = hints.iter().filter(|h| h.section.is_none() && h.group).copied().collect();
    let commands: Vec<Hint> = hints.iter().filter(|h| h.section.is_none() && !h.group).copied().collect();
    let plain = named.is_empty() && groups.is_empty();
    if !groups.is_empty() {
        out.push(("Groups".to_string(), groups));
    }
    if !commands.is_empty() {
        out.push((if plain { String::new() } else if named.is_empty() { "Commands".to_string() } else { "More".to_string() }, commands));
    }
    out
}

impl App {
    /// What the menu would show now, whether or not it's open yet: the
    /// local leader's, the leader's, or Vim's own prefix's.
    pub(super) fn which_key_pending(&self) -> Option<Pending> {
        let spc = KeyPress::char(' ');
        let (path, title, hints, source) = if let Some(local) = &self.local_matcher {
            let mut path = vec![spc, KeyPress::char('m')];
            path.extend_from_slice(local.path());
            let title = local.label().map(str::to_string).unwrap_or_else(|| self.local_contexts().iter().map(|c| c.name()).collect::<Vec<_>>().join(" · "));
            (path, title, local.pending_hints(), Source::Local)
        } else if self.leader_matcher.is_pending() {
            let title = self.leader_matcher.label().unwrap_or("leader").to_string();
            (self.leader_matcher.path().to_vec(), title, self.leader_matcher.pending_hints(), Source::Leader)
        } else {
            let path = self.vim.pending_path();
            let title = self.vim.pending_label().map(str::to_string).unwrap_or_else(|| {
                match path.first().map(|k| k.code) {
                    Some(KeyCode::Char('g')) => "go".to_string(),
                    Some(KeyCode::Char('z')) => "fold & view".to_string(),
                    Some(KeyCode::Char(']')) => "next".to_string(),
                    Some(KeyCode::Char('[')) => "previous".to_string(),
                    _ => "then".to_string(),
                }
            });
            (path, title, self.vim.pending_hints(), Source::Vim)
        };
        (!hints.is_empty()).then_some(Pending { path, title, hints, source })
    }

    /// Whether the menu is open this frame: once a sequence has waited
    /// `which_key.delay_ms`, and from then on until it ends.
    pub(super) fn which_key_open(&mut self, now: Instant) -> bool {
        let pending = self.which_key_pending();
        let Some(pending) = pending else {
            self.motion.which_key_since = None;
            self.motion.which_key_shown = false;
            self.motion.which_key_page = 0;
            self.motion.which_key_path.clear();
            return false;
        };
        if pending.path != self.motion.which_key_path {
            self.motion.which_key_path = pending.path.clone();
            self.motion.which_key_page = 0;
        }
        let since = *self.motion.which_key_since.get_or_insert(now);
        if !self.motion.which_key_shown && now.saturating_duration_since(since) >= self.which_key_delay() {
            self.motion.which_key_shown = true;
        }
        self.motion.which_key_shown
    }

    pub(super) fn which_key_delay(&self) -> Duration {
        Duration::from_millis(self.config.polish.which_key_delay_ms.unwrap_or(250))
    }

    /// When a waiting menu is due to open, for the event loop's timer.
    pub(super) fn which_key_wake(&self) -> Option<Instant> {
        (!self.motion.which_key_shown).then_some(())?;
        self.motion.which_key_since.map(|since| since + self.which_key_delay())
    }

    /// Backspace in an open leader menu: back up one level, or out.
    /// Says whether it took the key.
    pub(super) fn which_key_back(&mut self, key: KeyPress) -> bool {
        if key != KeyPress::named(FenixNamedKey::Backspace) {
            return false;
        }
        if let Some(local) = self.local_matcher.as_mut() {
            if !local.back() && local.path().is_empty() {
                // Back from the local menu's root: to the leader's.
                self.local_matcher = None;
                self.leader_matcher.feed(KeyPress::char(' '));
            }
            return true;
        }
        if self.leader_matcher.is_pending() {
            self.leader_matcher.back();
            return true;
        }
        false
    }

    /// `Ctrl-n`/`Ctrl-p` in an open menu too big for one page.
    pub(super) fn which_key_paging(&mut self, key: KeyPress) -> bool {
        let forward = key == KeyPress::char('n').with_ctrl();
        let back = key == KeyPress::char('p').with_ctrl();
        if !(forward || back) || !self.motion.which_key_shown || self.motion.which_key_pages < 2 {
            return false;
        }
        let pages = self.motion.which_key_pages;
        self.motion.which_key_page = if forward { (self.motion.which_key_page + 1) % pages } else { (self.motion.which_key_page + pages - 1) % pages };
        true
    }

    /// The menu laid out in `area` (the window above the modeline): the
    /// leader's and the local leader's as the palette, Vim's as the hint
    /// bar, kept off the cursor's line at `cursor_y`.
    pub(super) fn which_key_card(&self, pending: &Pending, area: fenix_window::Rect, char_width: f32, line_height: f32, cursor_y: Option<f32>) -> palette::Card {
        let by_label = self.config.polish.which_key_order.as_deref() == Some("label");
        let page = self.motion.which_key_page;
        if pending.source == Source::Vim {
            let mut hints = pending.hints.clone();
            if by_label {
                hints.sort_by(|a, b| a.label.cmp(b.label));
            } else {
                hints.sort_by_key(|h| key_rank(&h.key));
            }
            return palette::hint_bar(&pending.path, &pending.title, &hints, self.theme, area, char_width, line_height, cursor_y, page);
        }
        let entry = |h: &Hint| (keymap::describe_keypress(&h.key), h.label.to_string(), h.group, h.count);
        let sections: Vec<(String, Vec<(String, String, bool, usize)>)> = sections(pending.hints.clone(), by_label).into_iter().map(|(name, hints)| (if name.is_empty() { "keys".to_string() } else { name }, hints.iter().map(entry).collect())).collect();
        // The chords run lately that go on from here: their remaining keys
        // and what they run.
        let here = pending.path.iter().map(keymap::describe_keypress).collect::<Vec<_>>().join(" ");
        let places = self.places();
        // Two commands of one name (`SPC SPC` and `SPC f f`, both "find
        // file") show once; each with its area's icon.
        let mut named = std::collections::HashSet::new();
        let recent: Vec<(String, String, String)> = if pending.source == Source::Leader {
            self.recent_commands
                .iter()
                .filter_map(|id| places.get(id.as_str()).map(|p| (id, p)))
                .filter_map(|(id, (keys, _, label))| keys.strip_prefix(&here).map(|rest| (rest.trim().to_string(), label.to_string(), palette::command_area(id))))
                .filter(|(rest, label, _)| !rest.is_empty() && named.insert(label.clone()))
                .take(4)
                .collect()
        } else {
            Vec::new()
        };
        let top = pending.path.len() == 1 && pending.source == Source::Leader;
        let placeholder = if top { format!("Type the next key, or : to search all {} commands by name", self.commands.iter().filter(|c| c.id != "commands.palette").count()) } else { "Type the next key".to_string() };

        let view = palette::View {
            prompt: here,
            prompt_is_key: true,
            query: String::new(),
            placeholder,
            chips: vec![(pending.title.clone(), true), (format!("{} keys", pending.hints.len()), false)],
            body: palette::Body::Grid { recent, sections },
            side: None,
            footer: if pending.path.len() > 1 { vec![("Esc".to_string(), "close".to_string()), ("Bksp".to_string(), "back".to_string())] } else { vec![("Esc".to_string(), "close".to_string())] },
            empty: String::new(),
        };
        palette::layout(&view, self.theme, area, char_width, line_height, page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(key: char, label: &'static str, group: bool) -> Hint {
        Hint { key: KeyPress::char(key), label, group, count: if group { 3 } else { 0 }, section: None }
    }

    #[test]
    fn keys_are_ordered_by_key_letters_first() {
        let hints = vec![hint('g', "git", true), hint('a', "agenda", true), hint('G', "graph", false), hint(',', "settings", false), hint(' ', "find file", false)];
        let s = sections(hints, false);
        assert_eq!(s[0].0, "Groups");
        assert_eq!(s[0].1.iter().map(|h| h.label).collect::<Vec<_>>(), vec!["agenda", "git"]);
        assert_eq!(s[1].0, "Commands");
        assert_eq!(s[1].1.iter().map(|h| h.label).collect::<Vec<_>>(), vec!["graph", "settings", "find file"]);
    }

    #[test]
    fn named_sections_come_in_their_order_then_the_rest() {
        let mut hints = vec![hint('a', "stage hunk", false), hint('g', "status page", false), hint('q', "close", false)];
        hints[0].section = Some("This change");
        hints[1].section = Some("View");
        let s = sections(hints, false);
        let names: Vec<&str> = s.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["View", "This change", "More"]);
    }

    #[test]
    fn a_plain_list_has_no_section_names() {
        let s = sections(vec![hint('g', "top", false), hint('d', "definition", false)], false);
        assert_eq!(s.len(), 1);
        assert!(s[0].0.is_empty());
        assert_eq!(s[0].1[0].label, "definition");
    }

    #[test]
    fn by_label_when_asked() {
        let s = sections(vec![hint('a', "zebra", false), hint('z', "apple", false)], true);
        assert_eq!(s[0].1[0].label, "apple");
    }

    fn leader(app: &mut App, keys: &[char]) {
        for &k in keys {
            app.leader_matcher.feed(KeyPress::char(k));
        }
    }

    fn card_text(card: &palette::Card) -> String {
        card.spans.iter().map(|s| s.0.as_str()).collect()
    }

    const AREA: fenix_window::Rect = fenix_window::Rect { x: 0.0, y: 0.0, w: 1600.0, h: 870.0 };

    /// The leader's menu is the palette: the keys so far on caps, the
    /// group's name, its sections in columns, the groups with counts.
    #[test]
    fn the_leaders_menu_is_the_palette() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ', 'g']);
        let pending = app.which_key_pending().expect("SPC g is pending");
        assert_eq!((pending.title.as_str(), pending.source), ("git", Source::Leader));
        let card = app.which_key_card(&pending, AREA, 9.0, 20.0, None);
        assert_eq!(card.style, palette::CardStyle::Floating);
        let text = card_text(&card);
        let input = text.lines().next().unwrap();
        assert!(input.contains("SPC g") && input.contains("git"), "{input}");
        assert_eq!(card.boxes.iter().filter(|b| b.kind == palette::BoxKind::Key && b.row == 0).count(), 2, "SPC and g on caps");
        assert!(text.contains("VIEW") && text.contains("BRANCH & REMOTE") && text.contains("THIS CHANGE"), "{text}");
        leader(&mut app, &[]);
        app.leader_matcher.cancel();
        leader(&mut app, &[' ']);
        let pending = app.which_key_pending().unwrap();
        let text = card_text(&app.which_key_card(&pending, AREA, 9.0, 20.0, None));
        assert!(text.contains("GROUPS") && text.contains("COMMANDS") && text.contains("git"), "{text}");
        assert!(text.contains(": to search all"), "its search is SPC :: {text}");
    }

    /// Vim's own prefixes show in the hint bar, not over the code.
    #[test]
    fn vims_prefixes_show_in_the_hint_bar() {
        let mut app = App::with_file(None);
        app.test_dispatch_key(KeyPress::char('g'));
        let pending = app.which_key_pending().expect("g is pending");
        assert_eq!(pending.source, Source::Vim);
        let card = app.which_key_card(&pending, AREA, 9.0, 20.0, Some(100.0));
        assert!(matches!(card.style, palette::CardStyle::Bar { grows_up: true }));
        assert!(card_text(&card).contains("go"));
    }

    #[test]
    fn a_small_window_pages_instead_of_hiding() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ']);
        let pending = app.which_key_pending().unwrap();
        let card = app.which_key_card(&pending, fenix_window::Rect { x: 0.0, y: 0.0, w: 400.0, h: 260.0 }, 9.0, 20.0, None);
        assert!(card.pages > 1);
        assert!(card_text(&card).contains("page 1 of"));
    }

    #[test]
    fn the_menu_waits_before_opening() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ']);
        let now = Instant::now();
        assert!(!app.which_key_open(now), "not straight away");
        assert!(app.which_key_open(now + Duration::from_millis(300)));
        // Deeper levels open at once.
        app.leader_matcher.feed(KeyPress::char('g'));
        assert!(app.which_key_open(now + Duration::from_millis(301)));
        app.leader_matcher.cancel();
        assert!(!app.which_key_open(now + Duration::from_millis(302)));
        leader(&mut app, &[' ']);
        assert!(!app.which_key_open(now + Duration::from_millis(303)), "a new sequence waits again");
    }

    #[test]
    fn backspace_goes_up_a_level() {
        let mut app = App::with_file(None);
        leader(&mut app, &[' ', 'g']);
        assert!(app.which_key_back(KeyPress::named(FenixNamedKey::Backspace)));
        assert_eq!(app.leader_matcher.path(), &[KeyPress::char(' ')]);
        assert!(app.which_key_back(KeyPress::named(FenixNamedKey::Backspace)));
        assert!(!app.leader_matcher.is_pending());
    }
}
