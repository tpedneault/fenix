//! The welcome (`SPC h w`, and a fresh install's first screen): a few
//! steps, one at a time, for someone who has never used Fenix -- what it
//! is, how they'd like to edit (Vim keys, recommended, or standard keys),
//! how it looks, what to add, and where to learn the rest. Every choice
//! takes effect as it's made, so the steps after it already speak the
//! keys picked, and every one is a setting that can be changed later.
//! Each choice has a key on a cap, which a click presses too (`App`'s
//! `welcome_click`). `App` (`app/welcome_host.rs`) runs what it asks for.

use crate::page::{fit, frame, wrap, CardLook, Chip, Grid, Key, Page, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Hello,
    Keys,
    Look,
    Extensions,
    Learn,
}

pub const STEPS: [Step; 5] = [Step::Hello, Step::Keys, Step::Look, Step::Extensions, Step::Learn];

impl Step {
    pub fn title(self) -> &'static str {
        match self {
            Step::Hello => "Hello",
            Step::Keys => "Keys",
            Step::Look => "Look",
            Step::Extensions => "Extensions",
            Step::Learn => "Learn",
        }
    }

    fn index(self) -> usize {
        STEPS.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// The themes offered on the Look step, by name: a default, a few dark
/// ones people know, and light ones. The rest are in the theme picker.
pub const THEMES: &[&str] = &["Orbit Dark", "Tokyo Night Storm", "Catppuccin Mocha", "One Dark", "Gruvbox Dark", "GitHub Light", "Catppuccin Latte", "Solarized Light"];

/// A pack the Extensions step names: what it is, what it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub name: String,
    pub about: String,
}

pub struct WelcomePage {
    pub step: Step,
    /// The key scheme in use: `vim` or `standard`.
    pub keys: &'static str,
    /// The theme in use, by name.
    pub theme: String,
    /// How many themes there are, for the picker's mention.
    pub theme_count: usize,
    /// The text size in use.
    pub font_size: f32,
    /// The packs to mention.
    pub packs: Vec<Pack>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    /// The welcome is done (or skipped): on to Fenix.
    Finish,
    Keys(&'static str),
    Theme(&'static str),
    /// The text a step bigger (1) or smaller (-1).
    FontSize(i32),
    /// The extensions' packs, to pick from.
    Extensions,
    Tutorial,
    KeysPage,
    OpenProject,
}

impl WelcomePage {
    pub fn new(keys: &'static str, theme: String, theme_count: usize, font_size: f32, packs: Vec<Pack>) -> Self {
        WelcomePage { step: Step::Hello, keys, theme, theme_count, font_size, packs }
    }

    fn go(&mut self, by: isize) {
        let at = (self.step.index() as isize + by).clamp(0, STEPS.len() as isize - 1);
        self.step = STEPS[at as usize];
    }

    pub fn key(&mut self, key: Key) -> Action {
        match key {
            Key::Enter | Key::Right | Key::Space if self.step == Step::Learn => return Action::Finish,
            Key::Enter | Key::Right | Key::Space | Key::Tab => self.go(1),
            Key::Left | Key::Backspace | Key::BackTab => self.go(-1),
            Key::Escape | Key::Char('q') => return Action::Finish,
            _ => return self.step_key(key),
        }
        Action::None
    }

    fn step_key(&mut self, key: Key) -> Action {
        match (self.step, key) {
            (Step::Keys, Key::Char('v')) => Action::Keys("vim"),
            (Step::Keys, Key::Char('s')) => Action::Keys("standard"),
            (Step::Keys, Key::Up | Key::Down) => Action::Keys(if self.keys == "vim" { "standard" } else { "vim" }),
            (Step::Look, Key::Char(c @ '1'..='9')) => THEMES.get(c as usize - '1' as usize).map_or(Action::None, |t| Action::Theme(t)),
            (Step::Look, Key::Up | Key::Down) => {
                let at = THEMES.iter().position(|t| t.eq_ignore_ascii_case(&self.theme));
                let next = match (at, key) {
                    (None, _) => 0,
                    (Some(i), Key::Down) => (i + 1) % THEMES.len(),
                    (Some(i), _) => (i + THEMES.len() - 1) % THEMES.len(),
                };
                Action::Theme(THEMES[next])
            }
            (Step::Look, Key::Char('+' | '=')) => Action::FontSize(1),
            (Step::Look, Key::Char('-')) => Action::FontSize(-1),
            (Step::Extensions, Key::Char('x')) => Action::Extensions,
            (Step::Learn, Key::Char('t')) => Action::Tutorial,
            (Step::Learn, Key::Char('k')) => Action::KeysPage,
            (Step::Learn, Key::Char('o')) => Action::OpenProject,
            _ => Action::None,
        }
    }
}

/// What a click on a key's cap presses: the key written on it.
pub fn key_on_cap(cap: &str) -> Option<Key> {
    match cap.trim() {
        "Enter" | "↵" => Some(Key::Enter),
        "Esc" => Some(Key::Escape),
        "←" => Some(Key::Left),
        "→" => Some(Key::Right),
        cap => {
            let mut chars = cap.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Some(Key::Char(c)),
                _ => None,
            }
        }
    }
}

/// A choice: its key on a cap, its name, a tag (`recommended`), and what
/// it is in a few wrapped lines under the name. Returns the next line.
#[allow(clippy::too_many_arguments)]
fn choice(g: &mut Grid, line: usize, left: usize, width: usize, key: &str, name: &str, tag: Option<&str>, about: &str, on: bool) -> usize {
    let x = g.chip(line, left + 1, &format!(" {key} "), Role::Accent, if on { Chip::Chosen } else { Chip::Key });
    let end = g.put(line, x + 1, &fit(name, width.saturating_sub(x + 1 - left)), if on { Role::Title } else { Role::Text });
    if let Some(tag) = tag {
        if end + 2 + tag.chars().count() + 2 <= left + width {
            g.chip(line, end + 2, &format!(" {tag} "), Role::Good, Chip::Status(Role::Good));
        }
    }
    let mut at = line + 1;
    // A column short of the card's edge, which is drawn around it.
    for text in wrap(about, width.saturating_sub(x + 2 - left).max(10)) {
        g.put(at, x + 1, &text, Role::Muted);
        at += 1;
    }
    if on {
        g.card(line..at, left..left + width, CardLook::Selected);
    }
    at
}

/// Lays the page out `cols` wide.
pub fn layout(page: &WelcomePage, cols: usize) -> Page {
    let (left, width) = frame(cols, 76);
    let mut g = Grid::new();
    g.put(1, left, &fit("Welcome to Fenix", width), Role::Title);
    // Where you are: the steps, the one you're on lit.
    let mut x = left;
    let mut line = 2;
    for (i, step) in STEPS.iter().enumerate() {
        let label = format!("{} {}", i + 1, step.title());
        if x > left && x + label.chars().count() + 3 > left + width {
            line += 1;
            x = left;
        }
        if x > left {
            x = g.put(line, x, " · ", Role::Muted);
        }
        let role = if *step == page.step { Role::Accent } else if i < page.step.index() { Role::Text } else { Role::Muted };
        x = g.put(line, x, &label, role);
    }
    g.rule(line + 1, left..left + width);
    let mut line = line + 3;
    let para = |g: &mut Grid, line: &mut usize, text: &str, role: Role| {
        for l in wrap(text, width) {
            g.put(*line, left, &l, role);
            *line += 1;
        }
    };
    match page.step {
        Step::Hello => {
            para(&mut g, &mut line, "Fenix is an editor you drive from the keyboard. Text is edited with Vim's keys, and one key -- the leader -- opens a menu of everything else: files, projects, search, Git, agents, the debugger, terminals and settings.", Role::Text);
            line += 1;
            para(&mut g, &mut line, "A minute to make it yours:", Role::Text);
            for (i, what) in ["how you'd like to edit", "how it looks", "what to add to it", "where to learn the rest"].iter().enumerate() {
                g.put(line, left + 2, &format!("{}  {what}", i + 2), Role::Muted);
                line += 1;
            }
            line += 1;
            para(&mut g, &mut line, "Each choice is a setting, in use as you make it, and changed any time: SPC , opens the settings, and SPC h w brings this back.", Role::Muted);
        }
        Step::Keys => {
            para(&mut g, &mut line, "How would you like to edit?", Role::Text);
            line += 1;
            let vim = page.keys != "standard";
            line = choice(&mut g, line, left, width, "v", "Vim keys", Some("recommended"), "Modes: Normal to move and change, Insert to type. Operators and motions -- dw, ci\", 3j -- and Space for the menu of everything else. What Fenix is built around; the tutorial teaches it a lesson at a time.", vim) + 1;
            line = choice(&mut g, line, left, width, "s", "Standard keys", None, "Like VS Code: type at once, Ctrl+C, Ctrl+V and Ctrl+Z, Shift with the arrows selects, Alt+click adds a cursor, and Ctrl+K opens the same menu.", !vim) + 1;
            para(&mut g, &mut line, "Not sure? Start with Vim keys: SPC t k switches any time.", Role::Muted);
        }
        Step::Look => {
            para(&mut g, &mut line, "Pick a theme -- it changes as you choose.", Role::Text);
            line += 1;
            for (i, name) in THEMES.iter().enumerate() {
                let on = name.eq_ignore_ascii_case(&page.theme);
                let x = g.chip(line, left + 1, &format!(" {} ", i + 1), Role::Accent, if on { Chip::Chosen } else { Chip::Key });
                let end = g.put(line, x + 1, &fit(name, width.saturating_sub(x + 1 - left)), if on { Role::Title } else { Role::Text });
                if i == 0 && end + 10 <= left + width {
                    g.put(line, end + 2, "default", Role::Muted);
                }
                if on {
                    g.card(line..line + 1, left..left + width, CardLook::Selected);
                }
                line += 1;
            }
            line += 1;
            para(&mut g, &mut line, &format!("All {} themes are in SPC t p.", page.theme_count), Role::Muted);
            line += 1;
            let x = g.put(line, left, "Text size   ", Role::Text);
            let x = g.chip(line, x, " - ", Role::Accent, Chip::Key);
            let x = g.put(line, x + 1, &format!("{}", page.font_size.round() as i32), Role::Title);
            g.chip(line, x + 1, " + ", Role::Accent, Chip::Key);
        }
        Step::Extensions => {
            para(&mut g, &mut line, "Fenix starts with the core: editing, projects, search, Git, terminals, agents and the debugger. Extensions add languages and tools, and packs install a few that go together:", Role::Text);
            line += 1;
            let name_w = page.packs.iter().map(|p| p.name.chars().count()).max().unwrap_or(0).min(width / 3);
            for pack in &page.packs {
                g.put(line, left + 2, &fit(&pack.name, name_w), Role::Title);
                g.put(line, left + 4 + name_w, &fit(&pack.about, width.saturating_sub(name_w + 4)), Role::Muted);
                line += 1;
            }
            line += 1;
            line = choice(&mut g, line, left, width, "x", "Choose packs and extensions now", None, "Languages (Rust, Go, C++, Java...) are in the Browse tab. SPC h x any time.", false) + 1;
            para(&mut g, &mut line, "Or later: nothing is installed until you pick it.", Role::Muted);
        }
        Step::Learn => {
            para(&mut g, &mut line, "Where to go from here:", Role::Text);
            line += 1;
            let tutorial = if page.keys == "standard" { "Standard keys and Fenix's menu, a lesson at a time, practised in a real buffer. SPC h T any time." } else { "Vim's keys and Fenix's menu, a lesson at a time, practised in a real buffer. SPC h T any time." };
            line = choice(&mut g, line, left, width, "t", "The tutorial", Some("start here"), tutorial, false) + 1;
            line = choice(&mut g, line, left, width, "k", "Every key, on one page", None, "The chords and the menu's groups, for the keys you picked. SPC h ? any time.", false) + 1;
            line = choice(&mut g, line, left, width, "o", "Open a folder to work in", None, "Your projects, recent and new. SPC p p any time.", false) + 1;
            let palette = if page.keys == "standard" { "Ctrl+Shift+P" } else { "SPC :" };
            para(&mut g, &mut line, &format!("Lost? {palette} finds any command by its name, and SPC h k says what a key does."), Role::Muted);
        }
    }
    let next = if page.step == Step::Learn { "start using Fenix" } else { "next" };
    let mut keys: Vec<(&str, &str)> = Vec::new();
    if page.step != Step::Hello {
        keys.push(("←", "back"));
    }
    keys.push(("Enter", next));
    if page.step != Step::Learn {
        keys.push(("Esc", "skip the rest"));
    }
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> WelcomePage {
        WelcomePage::new("vim", "Orbit Dark".into(), 29, 14.0, vec![Pack { name: "Python".into(), about: "Python, Jupyter notebooks, CSV tables".into() }])
    }

    #[test]
    fn the_steps_go_forward_and_back_and_enter_on_the_last_finishes() {
        let mut p = page();
        assert_eq!(p.key(Key::Left), Action::None);
        assert_eq!(p.step, Step::Hello);
        for step in &STEPS[1..] {
            assert_eq!(p.key(Key::Enter), Action::None);
            assert_eq!(p.step, *step);
        }
        assert_eq!(p.key(Key::Backspace), Action::None);
        assert_eq!(p.step, Step::Extensions);
        p.key(Key::Right);
        assert_eq!(p.key(Key::Enter), Action::Finish);
        assert_eq!(page().key(Key::Escape), Action::Finish, "skipped from anywhere");
    }

    #[test]
    fn each_step_s_keys_ask_for_its_choices() {
        let mut p = page();
        p.step = Step::Keys;
        assert_eq!(p.key(Key::Char('s')), Action::Keys("standard"));
        assert_eq!(p.key(Key::Down), Action::Keys("standard"), "the other one");
        assert_eq!(p.key(Key::Char('t')), Action::None, "not this step's");
        p.step = Step::Look;
        assert_eq!(p.key(Key::Char('2')), Action::Theme("Tokyo Night Storm"));
        assert_eq!(p.key(Key::Up), Action::Theme("Solarized Light"), "round from the first");
        assert_eq!(p.key(Key::Char('+')), Action::FontSize(1));
        p.step = Step::Extensions;
        assert_eq!(p.key(Key::Char('x')), Action::Extensions);
        p.step = Step::Learn;
        assert_eq!(p.key(Key::Char('t')), Action::Tutorial);
        assert_eq!(p.key(Key::Char('k')), Action::KeysPage);
        assert_eq!(p.key(Key::Char('o')), Action::OpenProject);
    }

    #[test]
    fn vim_keys_are_recommended_and_the_choice_in_use_is_marked() {
        let mut p = page();
        p.step = Step::Keys;
        let laid = layout(&p, 100);
        let line = laid.text.lines().position(|l| l.contains("Vim keys")).unwrap();
        assert!(laid.text.lines().nth(line).unwrap().contains("recommended"), "{}", laid.text);
        assert!(laid.chips.iter().any(|(l, _, c)| *l == line && *c == Chip::Chosen), "in use");
        p.keys = "standard";
        let laid = layout(&p, 100);
        let line = laid.text.lines().position(|l| l.contains("Standard keys")).unwrap();
        assert!(laid.chips.iter().any(|(l, _, c)| *l == line && *c == Chip::Chosen));
    }

    #[test]
    fn every_step_keeps_inside_a_narrow_pane() {
        let mut p = page();
        for step in STEPS {
            p.step = step;
            for cols in [36, 40, 50, 60, 80, 120, 200] {
                let over = crate::page::overflow(&layout(&p, cols), cols);
                assert!(over.is_empty(), "{step:?} at {cols} cols: {over:#?}");
            }
        }
    }

    #[test]
    fn a_cap_s_key_is_what_a_click_on_it_presses() {
        assert_eq!(key_on_cap(" v "), Some(Key::Char('v')));
        assert_eq!(key_on_cap(" Enter "), Some(Key::Enter));
        assert_eq!(key_on_cap("←"), Some(Key::Left));
        assert_eq!(key_on_cap(" 12 "), None);
    }
}
