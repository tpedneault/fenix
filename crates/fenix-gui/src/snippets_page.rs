//! `SPC i S`: every snippet, by language -- the one you're typing in
//! first -- with where each comes from (built in, yours, the project's)
//! and, for the selected one, its trigger, what it's for, its file and a
//! preview with its fields marked. `n` makes one (from a Visual selection,
//! `SPC i n` fills the body in), `Enter` edits one's file, `c` copies one
//! to yours to change it, `p` into the project, `d` deletes one (to the
//! Recycle Bin), `t` tries it in the buffer you came from. A file that
//! doesn't parse is listed with the reason.

use std::path::PathBuf;

use fenix_snippets::Source;

use crate::page::{fit, frame, Grid, Key, Page, Popup, Role};

/// One snippet, as the page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub name: String,
    pub trigger: String,
    pub scopes: Vec<String>,
    pub source: Source,
    pub file: Option<PathBuf>,
    /// The file's text.
    pub text: String,
    /// What it inserts, fields at their defaults.
    pub preview: String,
    pub fields: usize,
    /// A closer layer has one with the same trigger and language.
    pub overridden: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Close,
    /// Make a new one, then open its file.
    Create { trigger: String, name: String, scopes: Vec<String>, body: String, project: bool },
    Edit(PathBuf),
    /// Copy one into yours (`project: false`) or the project's.
    Copy { item: Box<Item>, project: bool },
    Delete(PathBuf),
    Try(Box<Item>),
}

/// The new-snippet form: trigger, name, languages.
#[derive(Debug, Clone, PartialEq)]
struct Form {
    fields: [String; 3],
    at: usize,
    body: String,
    project: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Row {
    Heading(usize),
    Item(usize),
}

pub struct SnippetsPage {
    pub items: Vec<Item>,
    /// Files that don't parse, with why.
    pub broken: Vec<String>,
    /// The language of the buffer you came from.
    pub scope: String,
    pub has_project: bool,
    cursor: usize,
    filter: Option<String>,
    filtering: bool,
    form: Option<Form>,
    pub note: Option<(String, bool)>,
    armed_delete: bool,
}

/// How a language reads as a heading.
fn language(scope: &str) -> String {
    match scope {
        "*" => "All languages".to_string(),
        "c" => "C".to_string(),
        "cpp" => "C++".to_string(),
        other => {
            let mut chars = other.chars();
            chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
        }
    }
}

impl SnippetsPage {
    pub fn new(items: Vec<Item>, broken: Vec<String>, scope: String, has_project: bool) -> Self {
        SnippetsPage { items, broken, scope, has_project, cursor: 0, filter: None, filtering: false, form: None, note: None, armed_delete: false }
    }

    /// New contents, keeping the cursor on the same snippet when it's
    /// still there.
    pub fn refresh(&mut self, items: Vec<Item>, broken: Vec<String>) {
        let before = self.selected().map(|i| (i.trigger.clone(), i.source, i.scopes.clone()));
        self.items = items;
        self.broken = broken;
        if let Some(key) = before {
            if let Some(n) = self.selectable().iter().position(|&i| (self.items[i].trigger.clone(), self.items[i].source, self.items[i].scopes.clone()) == key) {
                self.cursor = n;
            }
        }
        self.cursor = self.cursor.min(self.selectable().len().saturating_sub(1));
    }

    /// Opens the new-snippet form with `body` already in it.
    pub fn new_from(&mut self, body: String) {
        self.form = Some(Form { fields: [String::new(), String::new(), self.scope.clone()], at: 0, body, project: false });
    }

    pub fn typing(&self) -> bool {
        self.filtering || self.form.is_some()
    }

    /// Puts the cursor on the first snippet `is` picks out.
    #[cfg(test)]
    pub fn select(&mut self, is: impl Fn(&Item) -> bool) -> bool {
        match self.selectable().iter().position(|&i| is(&self.items[i])) {
            Some(n) => {
                self.cursor = n;
                true
            }
            None => false,
        }
    }

    pub fn paste(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if self.filtering {
            self.filter.get_or_insert_with(String::new).push_str(&text);
        } else if let Some(form) = &mut self.form {
            form.fields[form.at].push_str(&text);
        }
    }

    /// The languages, the current one first, then "All languages", then
    /// the rest by name.
    fn groups(&self) -> Vec<String> {
        let mut scopes: Vec<String> = self.items.iter().map(|i| i.scopes.first().cloned().unwrap_or_else(|| "*".into())).collect();
        scopes.sort();
        scopes.dedup();
        scopes.sort_by_key(|s| (s != &self.scope, s != "*", s.clone()));
        scopes
    }

    fn shown(&self, item: &Item) -> bool {
        match self.filter.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) => {
                let f = f.to_lowercase();
                item.trigger.to_lowercase().contains(&f) || item.name.to_lowercase().contains(&f) || item.scopes.iter().any(|s| s.contains(&f))
            }
            None => true,
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (g, scope) in self.groups().iter().enumerate() {
            let mut members: Vec<usize> = (0..self.items.len())
                .filter(|&i| self.items[i].scopes.first().map(String::as_str).unwrap_or("*") == scope && self.shown(&self.items[i]))
                .collect();
            members.sort_by_key(|&i| (self.items[i].trigger.clone(), std::cmp::Reverse(self.items[i].source)));
            if !members.is_empty() {
                rows.push(Row::Heading(g));
                rows.extend(members.into_iter().map(Row::Item));
            }
        }
        rows
    }

    fn selectable(&self) -> Vec<usize> {
        self.rows().into_iter().filter_map(|r| if let Row::Item(i) = r { Some(i) } else { None }).collect()
    }

    pub fn selected(&self) -> Option<&Item> {
        self.selectable().get(self.cursor).map(|&i| &self.items[i])
    }

    pub fn key(&mut self, key: Key) -> Action {
        if key != Key::Char('d') {
            self.armed_delete = false;
        }
        if self.filtering {
            match key {
                Key::Escape => {
                    self.filtering = false;
                    self.filter = None;
                }
                Key::Enter => self.filtering = false,
                Key::Backspace => {
                    if let Some(f) = &mut self.filter {
                        f.pop();
                    }
                }
                Key::Char(c) => self.filter.get_or_insert_with(String::new).push(c),
                Key::Space => self.filter.get_or_insert_with(String::new).push(' '),
                _ => {}
            }
            self.cursor = 0;
            return Action::None;
        }
        if let Some(mut form) = self.form.take() {
            match key {
                Key::Escape => return Action::None,
                Key::Backspace => {
                    form.fields[form.at].pop();
                }
                Key::Char(c) => form.fields[form.at].push(c),
                Key::Space if form.at != 0 => form.fields[form.at].push(' '),
                Key::Tab | Key::Down => form.at = (form.at + 1) % 3,
                Key::BackTab | Key::Up => form.at = (form.at + 2) % 3,
                Key::Enter if form.at < 2 => form.at += 1,
                Key::Enter => {
                    let trigger = form.fields[0].trim().to_string();
                    if trigger.is_empty() || trigger.chars().any(char::is_whitespace) {
                        self.note = Some(("a trigger is one word: what you type before Tab".into(), true));
                        form.at = 0;
                        self.form = Some(form);
                        return Action::None;
                    }
                    let name = Some(form.fields[1].trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| trigger.clone());
                    let scopes: Vec<String> = form.fields[2].split([',', ' ']).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect();
                    let scopes = if scopes.is_empty() { vec!["*".to_string()] } else { scopes };
                    self.note = None;
                    return Action::Create { trigger, name, scopes, body: form.body, project: form.project };
                }
                _ => {}
            }
            self.form = Some(form);
            return Action::None;
        }
        let n = self.selectable().len();
        match key {
            Key::Down | Key::Char('j') => self.cursor = (self.cursor + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            Key::Char('/') => {
                self.filtering = true;
                self.filter = Some(String::new());
            }
            Key::Escape if self.filter.is_some() => self.filter = None,
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('n') => self.new_from(String::new()),
            Key::Char('N') if self.has_project => {
                self.new_from(String::new());
                if let Some(form) = &mut self.form {
                    form.project = true;
                }
            }
            Key::Enter => match self.selected() {
                Some(Item { file: Some(file), .. }) => return Action::Edit(file.clone()),
                Some(_) => self.note = Some(("a built-in one -- c copies it to yours, where you can change it".into(), false)),
                None => {}
            },
            Key::Char('c') => {
                if let Some(item) = self.selected() {
                    if item.source == Source::User {
                        self.note = Some(("it's yours already -- Enter edits it".into(), false));
                    } else {
                        return Action::Copy { item: Box::new(item.clone()), project: false };
                    }
                }
            }
            Key::Char('p') => match (self.selected(), self.has_project) {
                (_, false) => self.note = Some(("no project open -- open a file in one first".into(), true)),
                (Some(item), true) if item.source == Source::Project => self.note = Some(("it's the project's already".into(), false)),
                (Some(item), true) => return Action::Copy { item: Box::new(item.clone()), project: true },
                (None, _) => {}
            },
            Key::Char('d') => match self.selected().map(|i| i.file.clone()) {
                Some(Some(file)) => {
                    if !self.armed_delete {
                        self.armed_delete = true;
                        self.note = Some(("d again deletes it (to the Recycle Bin)".into(), false));
                        return Action::None;
                    }
                    self.armed_delete = false;
                    return Action::Delete(file);
                }
                Some(_) => self.note = Some(("a built-in one can't be deleted -- [snippets] builtin = false hides them all".into(), false)),
                None => {}
            },
            Key::Char('t') => {
                if let Some(item) = self.selected() {
                    return Action::Try(Box::new(item.clone()));
                }
            }
            _ => {}
        }
        Action::None
    }
}

pub fn layout(page: &SnippetsPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 132);
    let mut g = Grid::new();
    let x = g.put(1, left, "Snippets", Role::Title) + 2;
    g.put(1, x, &format!("for {}", language(&page.scope)), Role::Muted);
    let count = format!("{} snippets", page.items.len());
    g.put(1, (left + width).saturating_sub(count.chars().count()), &count, Role::Muted);
    let mut y = 2;
    let filter = match (&page.filter, page.filtering) {
        (Some(f), true) => format!("/ {f}▏"),
        (Some(f), false) if !f.is_empty() => format!("/ {f}  (Esc clears)"),
        _ => "/ filter".to_string(),
    };
    let end = g.put(y, left, &filter, if page.filtering { Role::Title } else { Role::Muted });
    if page.filtering {
        g.panels.push((y, left..end.max(left + 30)));
    }
    y += 1;
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    for broken in page.broken.iter().take(3) {
        g.put(y, left, &fit(&format!("✗ {broken}"), width), Role::Bad);
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;
    let top = y;

    let list_w = 52.min(width / 2);
    let groups = page.groups();
    let selected = page.selectable().get(page.cursor).copied();
    let mut focus_line = top;
    for row in page.rows() {
        match row {
            Row::Heading(gi) => {
                if y > top {
                    y += 1;
                }
                let n = page.items.iter().filter(|i| i.scopes.first().map(String::as_str).unwrap_or("*") == groups[gi] && page.shown(i)).count();
                g.heading(y, left, list_w, &format!("{} {n}", language(&groups[gi])));
            }
            Row::Item(i) => {
                let item = &page.items[i];
                let x = g.put(y, left + 2, &fit(&item.trigger, 12), if selected == Some(i) { Role::Title } else { Role::Text }) + 1;
                let x = x.max(left + 16);
                g.put(y, x, &fit(&item.name, list_w.saturating_sub(x - left + 11)), if item.overridden { Role::Muted } else { Role::Text });
                let tag = if item.overridden { "overridden" } else { item.source.label() };
                let role = match (item.overridden, item.source) {
                    (true, _) => Role::Muted,
                    (_, Source::Project) => Role::Good,
                    (_, Source::User) => Role::Accent,
                    (_, Source::BuiltIn) => Role::Muted,
                };
                g.put(y, (left + list_w).saturating_sub(tag.chars().count()), tag, role);
                if selected == Some(i) {
                    g.focus(y, left..left + list_w);
                    focus_line = y;
                }
            }
        }
        y += 1;
    }
    if page.items.is_empty() {
        g.put(top, left, "No snippets yet -- n makes one.", Role::Muted);
    }

    // The selected one, on the right.
    let dx = left + list_w + 3;
    let dw = width.saturating_sub(list_w + 3);
    if let Some(item) = page.selected() {
        let mut y = top;
        let rows: [(&str, String); 4] = [
            ("Trigger", format!("{}  · Tab after it, or SPC i s", item.trigger)),
            ("Name", item.name.clone()),
            ("For", item.scopes.iter().map(|s| language(s)).collect::<Vec<_>>().join(", ")),
            ("File", item.file.as_ref().map(|f| f.display().to_string()).unwrap_or_else(|| "built into Fenix".into())),
        ];
        for (label, value) in rows {
            g.put(y, dx, label, Role::Muted);
            g.put(y, dx + 9, &fit(&value, dw.saturating_sub(9)), Role::Text);
            y += 1;
        }
        y += 1;
        g.heading(y, dx, dw, &format!("Preview · {}", match item.fields {
            0 => "no fields".to_string(),
            1 => "1 field".to_string(),
            n => format!("{n} fields, Tab moves between them"),
        }));
        y += 1;
        for line in item.preview.lines().take(16) {
            g.put(y, dx + 2, &fit(line, dw.saturating_sub(2)), Role::Text);
            y += 1;
        }
        if item.overridden {
            y += 1;
            g.put(y, dx, &fit("A snippet closer to you has the same trigger, so this one isn't used.", dw), Role::Warn);
        }
    }

    // The new-snippet form floats by the list.
    if let Some(form) = &page.form {
        let labels = ["Trigger", "Name", "For"];
        let hints = ["what you type before Tab", "what the list calls it", "languages, like tcl, python; empty for all"];
        let mut rows = vec![vec![(format!("New snippet{}", if form.project { " · this project's" } else { "" }), Role::Title)], Vec::new()];
        for (i, label) in labels.iter().enumerate() {
            let on = i == form.at;
            let value = if on { format!("{}▏", form.fields[i]) } else { form.fields[i].clone() };
            rows.push(vec![(format!("{label:<9}"), if on { Role::Accent } else { Role::Muted }), (format!("{value:<34}"), if on { Role::Title } else { Role::Text })]);
        }
        // Only the field being typed in explains itself, on a line of its
        // own: text after the caret glyph wouldn't line up.
        rows.push(vec![(format!("{:9}{}", "", hints[form.at]), Role::Muted)]);
        if !form.body.is_empty() {
            rows.push(Vec::new());
            rows.push(vec![(format!("Body: {} lines from your selection", form.body.lines().count().max(1)), Role::Muted)]);
        }
        rows.push(Vec::new());
        rows.push(vec![("Tab next field · Enter makes it and opens its file · Esc cancels".into(), Role::Muted)]);
        g.popup = Some(Popup { line: focus_line, col: left + 2, rows });
    }

    let keys: Vec<(&str, &str)> = if page.filtering {
        vec![("Enter", "done"), ("Esc", "clear")]
    } else if page.form.is_some() {
        vec![("Tab", "next field"), ("Enter", "make it"), ("Esc", "cancel")]
    } else {
        vec![("n", "new"), ("Enter", "edit"), ("c", "copy to yours"), ("p", "copy to the project"), ("d", "delete"), ("t", "try it"), ("/", "filter"), ("q", "close")]
    };
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(trigger: &str, scope: &str, source: Source) -> Item {
        Item {
            name: format!("{trigger} snippet"),
            trigger: trigger.into(),
            scopes: vec![scope.into()],
            source,
            file: (source != Source::BuiltIn).then(|| PathBuf::from(format!("/s/{scope}/{trigger}.snippet"))),
            text: String::new(),
            preview: "proc name {args} {\n}".into(),
            fields: 2,
            overridden: false,
        }
    }

    fn page() -> SnippetsPage {
        let mut overridden = item("proc", "tcl", Source::BuiltIn);
        overridden.overridden = true;
        let items = vec![item("header", "*", Source::BuiltIn), overridden, item("proc", "tcl", Source::User), item("tc", "tcl", Source::Project), item("fn", "rust", Source::User)];
        SnippetsPage::new(items, vec!["/s/tcl/tm.snippet: missing # key:".into()], "tcl".into(), true)
    }

    #[test]
    fn the_current_language_comes_first_and_each_says_where_it_comes_from() {
        let p = page();
        let text = layout(&p, 130).text;
        let tcl = text.find("TCL 3").unwrap();
        let all = text.find("ALL LANGUAGES 1").unwrap();
        let rust = text.find("RUST 1").unwrap();
        assert!(tcl < all && all < rust, "{text}");
        assert!(text.contains("overridden") && text.contains("yours") && text.contains("project") && text.contains("built-in"), "{text}");
        assert!(text.contains("✗ /s/tcl/tm.snippet: missing # key:"), "a broken file says why");
        assert!(text.contains("PREVIEW · 2 FIELDS"), "{text}");
    }

    #[test]
    fn a_built_in_one_is_copied_to_be_changed_and_yours_are_edited_and_deleted() {
        let mut p = page();
        // tcl: proc (yours), proc (built-in, overridden), tc (project).
        assert_eq!(p.selected().unwrap().source, Source::User);
        assert_eq!(p.key(Key::Enter), Action::Edit(PathBuf::from("/s/tcl/proc.snippet")));
        p.key(Key::Char('j'));
        assert_eq!(p.selected().unwrap().source, Source::BuiltIn);
        assert_eq!(p.key(Key::Enter), Action::None);
        assert!(p.note.as_ref().unwrap().0.contains("c copies it"));
        assert!(matches!(p.key(Key::Char('c')), Action::Copy { project: false, .. }));
        p.key(Key::Char('k'));
        assert_eq!(p.key(Key::Char('d')), Action::None, "asks first");
        assert_eq!(p.key(Key::Char('d')), Action::Delete(PathBuf::from("/s/tcl/proc.snippet")));
        assert!(matches!(p.key(Key::Char('t')), Action::Try(_)));
    }

    #[test]
    fn a_new_one_is_made_from_the_form_with_a_selection_as_its_body() {
        let mut p = page();
        p.new_from("puts $x\n".into());
        assert!(layout(&p, 130).popup.unwrap().text().contains("Body: 1 lines from your selection"));
        for c in "say".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Tab);
        p.key(Key::Tab);
        assert_eq!(p.key(Key::Enter), Action::Create { trigger: "say".into(), name: "say".into(), scopes: vec!["tcl".into()], body: "puts $x\n".into(), project: false });
    }

    #[test]
    fn a_trigger_is_one_word_and_can_not_be_empty() {
        let mut p = page();
        p.key(Key::Char('n'));
        for c in ['a', ' ', 'b'] {
            p.key(if c == ' ' { Key::Space } else { Key::Char(c) });
        }
        p.key(Key::Enter);
        p.key(Key::Enter);
        let Action::Create { trigger, .. } = p.key(Key::Enter) else { panic!() };
        assert_eq!(trigger, "ab", "a space isn't typed into a trigger");
        p.key(Key::Char('n'));
        p.key(Key::Enter);
        p.key(Key::Enter);
        assert_eq!(p.key(Key::Enter), Action::None);
        assert!(p.note.as_ref().unwrap().0.contains("one word"));
    }

    #[test]
    fn the_filter_matches_triggers_names_and_languages() {
        let mut p = page();
        p.key(Key::Char('/'));
        for c in "rust".chars() {
            p.key(Key::Char(c));
        }
        let text = layout(&p, 130).text;
        assert!(text.contains("fn snippet") && !text.contains("header snippet"), "{text}");
    }
}
