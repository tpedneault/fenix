//! `SPC k i`, or `i` on a telecommand: the insert form. Every variable
//! argument at once, each with its default filled in, its type, unit and
//! what it may be; a status parameter cycles through its texts with `h`/
//! `l`; a counter (`CDF_GRPSIZE`) repeats the arguments after it as many
//! times as its value says, `+`/`-` changing it. The command, rendered
//! from the project's templates, updates under the form as values change,
//! with a warning beside any value outside what the MIB allows -- a
//! warning never stops the insert. `Ctrl-Enter` (or `I`) inserts it
//! where the form was opened.

use std::collections::HashMap;

use fenix_mib::telecommand::{self, ParamDomain, TcParameter};
use fenix_mib::{DefRef, MibSet, Row};

use crate::mib_page::{kind_role, MibKey};
use crate::page::{fit, fit_tail, frame, wrap, Grid, Key, Page, Popup, Role};

/// How a command is written: the project's templates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Templates {
    pub command: String,
    pub argument: String,
    pub separator: String,
}

struct Param {
    tc: TcParameter,
    domain: ParamDomain,
    /// How many parameters after it repeat (`CDF_GRPSIZE`).
    group: usize,
}

/// One field of the form: a parameter, in a repetition of each group
/// around it (`path`, 1-based, outermost first).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slot {
    param: usize,
    path: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    /// Open the telecommand's page.
    Open(DefRef),
    /// Insert this text.
    Insert(String),
    /// Copy the telecommand's packet, as a literal for the file.
    CopyBytes,
    /// Open the packet in the inspector.
    DecodeBytes,
}

pub struct InsertForm {
    pub key: MibKey,
    pub tc: DefRef,
    ccf: Row,
    root_label: String,
    name: String,
    description: String,
    pus: String,
    apid: String,
    params: Vec<Param>,
    fixed: usize,
    values: HashMap<String, String>,
    focus: usize,
    editing: bool,
    /// The texts menu, and its row.
    menu: Option<usize>,
    help: bool,
    templates: Templates,
    pub note: Option<(String, bool)>,
    /// A line being edited: the call is put back in its place.
    pub replacing: bool,
    /// The Bytes section is shown.
    pub show_bytes: bool,
    /// The sequence count built packets get.
    pub seq: u16,
}

/// How many parameters `slots` slots after `start` take up: a nested
/// counter is one slot, plus everything its own group holds.
fn span(params: &[Param], start: usize, slots: usize) -> usize {
    let mut i = start;
    for _ in 0..slots {
        if i >= params.len() {
            break;
        }
        let g = params[i].group;
        i += 1;
        if g > 0 {
            i += span(params, i, g);
        }
    }
    i - start
}

fn key(params: &[Param], slot: &Slot) -> String {
    let path: Vec<String> = slot.path.iter().map(|n| n.to_string()).collect();
    format!("{}@{}", params[slot.param].tc.name, path.join("."))
}

impl InsertForm {
    /// The form for `tc`, its values from `remembered` (a previous insert
    /// of the same telecommand) where there are some, else the MIB's
    /// defaults.
    pub fn new(set: &MibSet, key: MibKey, tc: DefRef, templates: Templates, remembered: Option<&HashMap<String, String>>, apid_hex: bool) -> Self {
        let e = set.get(tc);
        let all = telecommand::tc_parameters(set.index(), &e.row);
        let fixed = all.iter().filter(|p| p.fixed).count();
        let params: Vec<Param> = all
            .into_iter()
            .filter(|p| !p.fixed && !p.name.is_empty())
            .map(|p| {
                let domain = telecommand::parameter_domain(set.index(), &p);
                let group = fenix_mib::types::parse_int(p.cdf.clean("CDF_GRPSIZE")).unwrap_or(0).max(0) as usize;
                Param { tc: p, domain, group }
            })
            .collect();
        let mut form = InsertForm {
            key,
            tc,
            ccf: e.row.clone(),
            root_label: set.root_label(e.root).to_string(),
            name: e.name.clone(),
            description: e.description.clone(),
            pus: format!("{},{}", e.row.clean("CCF_TYPE"), e.row.clean("CCF_STYPE")),
            apid: fenix_mib::types::apid(e.row.clean("CCF_APID"), apid_hex),
            params,
            fixed,
            values: HashMap::new(),
            focus: 0,
            editing: false,
            menu: None,
            help: false,
            templates,
            note: None,
            replacing: false,
            show_bytes: false,
            seq: 0,
        };
        if let Some(r) = remembered {
            form.values = r.clone();
        }
        form
    }

    pub fn typing(&self) -> bool {
        self.editing
    }

    /// The telecommand's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn claims_space(&self) -> bool {
        self.editing
    }

    pub fn paste(&mut self, text: &str) {
        if self.editing {
            let k = self.focus_key();
            if let Some(k) = k {
                self.values.entry(k).or_default().extend(text.chars().filter(|c| !c.is_control()));
            }
        }
    }

    /// Sets values by parameter name, in order -- a call being edited,
    /// read back from its line. A counter's value comes before the
    /// repetitions it opens, so each is placed as the form grows.
    pub fn fill(&mut self, args: &[(String, String)]) {
        self.values.clear();
        let mut used = vec![false; args.len()];
        loop {
            let slots = self.slots();
            let mut changed = false;
            for slot in &slots {
                let k = key(&self.params, slot);
                if self.values.contains_key(&k) {
                    continue;
                }
                let name = &self.params[slot.param].tc.name;
                if let Some(i) = (0..args.len()).find(|&i| !used[i] && &args[i].0 == name) {
                    used[i] = true;
                    self.values.insert(k, args[i].1.clone());
                    changed = true;
                    break;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Every field, as the counters' values expand the groups.
    fn slots(&self) -> Vec<Slot> {
        let mut out = Vec::new();
        self.expand(0, self.params.len(), Vec::new(), &mut out);
        out
    }

    fn expand(&self, start: usize, end: usize, path: Vec<usize>, out: &mut Vec<Slot>) {
        let mut i = start;
        while i < end {
            let slot = Slot { param: i, path: path.clone() };
            let g = self.params[i].group;
            let count = if g > 0 { fenix_mib::types::parse_int(&self.value(&slot)).unwrap_or(0).clamp(0, 99) as usize } else { 0 };
            out.push(slot);
            if g > 0 {
                let s = span(&self.params, i + 1, g);
                for rep in 1..=count {
                    let mut p = path.clone();
                    p.push(rep);
                    self.expand(i + 1, (i + 1 + s).min(end), p, out);
                }
                i += 1 + s;
            } else {
                i += 1;
            }
        }
    }

    fn value(&self, slot: &Slot) -> String {
        self.values.get(&key(&self.params, slot)).cloned().unwrap_or_else(|| self.params[slot.param].domain.default.clone())
    }

    fn focus_key(&self) -> Option<String> {
        self.slots().get(self.focus).map(|s| key(&self.params, s))
    }

    /// Each field's `(name, value)`, in order: what the command takes.
    pub fn arguments(&self) -> Vec<(String, String)> {
        self.slots().iter().map(|s| (self.params[s.param].tc.name.clone(), self.value(s))).collect()
    }

    /// The values as typed, to open with next time.
    pub fn values(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for s in self.slots() {
            out.insert(key(&self.params, &s), self.value(&s));
        }
        out
    }

    pub fn rendered(&self) -> String {
        let t = &self.templates;
        telecommand::render_telecommand(&t.command, &t.argument, &t.separator, &self.ccf, &self.root_label, &self.arguments())
    }

    fn warnings_for(&self, slot: &Slot) -> Vec<String> {
        let p = &self.params[slot.param];
        let v = self.value(slot);
        if v.trim().is_empty() {
            return vec![format!("{}: no value", p.tc.name)];
        }
        telecommand::validate_argument(&p.tc, &v, &p.domain)
    }

    pub fn warnings(&self) -> usize {
        self.slots().iter().map(|s| self.warnings_for(s).len()).sum()
    }

    fn set_focused(&mut self, value: String) {
        if let Some(k) = self.focus_key() {
            self.values.insert(k, value);
        }
    }

    fn cycle(&mut self, by: isize) {
        let slots = self.slots();
        let Some(slot) = slots.get(self.focus) else { return };
        let p = &self.params[slot.param];
        if p.group > 0 {
            let n = fenix_mib::types::parse_int(&self.value(slot)).unwrap_or(0);
            self.set_focused((n + by as i64).clamp(0, 99).to_string());
            return;
        }
        let aliases = &p.domain.aliases;
        if aliases.is_empty() {
            self.note = Some(("h l change a status value or a count; Enter types this one".into(), false));
            return;
        }
        let at = aliases.iter().position(|a| *a == self.value(slot)).map(|i| i as isize).unwrap_or(-1);
        let next = (at + by).rem_euclid(aliases.len() as isize) as usize;
        self.set_focused(aliases[next].clone());
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.note = None;
        if self.help {
            self.help = false;
            return Action::None;
        }
        let n = self.slots().len();
        if let Some(at) = self.menu {
            let aliases = self.slots().get(self.focus).map(|s| self.params[s.param].domain.aliases.clone()).unwrap_or_default();
            match key {
                Key::Escape | Key::Char('q') => self.menu = None,
                Key::Down | Key::Char('j') => self.menu = Some((at + 1).min(aliases.len().saturating_sub(1))),
                Key::Up | Key::Char('k') => self.menu = Some(at.saturating_sub(1)),
                Key::Char(c @ '1'..='9') => {
                    if let Some(a) = aliases.get(c as usize - '1' as usize) {
                        self.set_focused(a.clone());
                        self.menu = None;
                    }
                }
                Key::Enter | Key::Space => {
                    if let Some(a) = aliases.get(at) {
                        self.set_focused(a.clone());
                    }
                    self.menu = None;
                }
                _ => {}
            }
            return Action::None;
        }
        if self.editing {
            match key {
                Key::Enter | Key::Escape => self.editing = false,
                Key::Tab | Key::Down => {
                    self.editing = false;
                    self.focus = (self.focus + 1).min(n.saturating_sub(1));
                }
                Key::BackTab | Key::Up => {
                    self.editing = false;
                    self.focus = self.focus.saturating_sub(1);
                }
                Key::Backspace => {
                    let slots = self.slots();
                    if let Some(s) = slots.get(self.focus) {
                        let mut v = self.value(s);
                        v.pop();
                        self.set_focused(v);
                    }
                }
                Key::Char(c) => {
                    let slots = self.slots();
                    if let Some(s) = slots.get(self.focus) {
                        let mut v = self.value(s);
                        v.push(c);
                        self.set_focused(v);
                    }
                }
                Key::Space => {
                    let slots = self.slots();
                    if let Some(s) = slots.get(self.focus) {
                        let v = self.value(s) + " ";
                        self.set_focused(v);
                    }
                }
                Key::CtrlEnter => {
                    self.editing = false;
                    return Action::Insert(self.rendered());
                }
                _ => {}
            }
            return Action::None;
        }
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('?') => self.help = true,
            Key::Down | Key::Char('j') | Key::Tab => self.focus = (self.focus + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') | Key::BackTab => self.focus = self.focus.saturating_sub(1),
            Key::Char('g') => self.focus = 0,
            Key::Char('G') => self.focus = n.saturating_sub(1),
            Key::Left | Key::Char('h') | Key::Char('-') => self.cycle(-1),
            Key::Right | Key::Char('l') | Key::Char('+') => self.cycle(1),
            Key::Enter => {
                let slots = self.slots();
                if let Some(s) = slots.get(self.focus) {
                    let aliases = &self.params[s.param].domain.aliases;
                    if aliases.is_empty() {
                        self.editing = true;
                    } else {
                        self.menu = Some(aliases.iter().position(|a| *a == self.value(s)).unwrap_or(0));
                    }
                }
            }
            Key::Char('i') | Key::Char('a') => self.editing = n > 0,
            Key::Char('c') => {
                if n > 0 {
                    self.set_focused(String::new());
                    self.editing = true;
                }
            }
            Key::Char('R') => {
                self.values.clear();
                self.note = Some(("back to the MIB's defaults".into(), false));
            }
            Key::Char('d') => return Action::Open(self.tc),
            Key::Char('b') => self.show_bytes = !self.show_bytes,
            Key::Char('s') => {
                self.seq = (self.seq + 1) & 0x3FFF;
                self.show_bytes = true;
            }
            Key::Char('S') => self.seq = 0,
            Key::Char('y') => return Action::CopyBytes,
            Key::Char('D') => return Action::DecodeBytes,
            Key::Char('I') | Key::CtrlEnter => return Action::Insert(self.rendered()),
            _ => {}
        }
        Action::None
    }
}

pub fn title(form: &InsertForm) -> String {
    format!("*insert: {}*", form.name)
}

/// The form, with its packet's bytes when the Bytes section is shown
/// (the host builds them from the MIB).
pub fn layout(form: &InsertForm, bytes: Option<&Result<Vec<u8>, String>>, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    g.put(1, left, if form.replacing { "Edit a telecommand call" } else { "Insert a telecommand" }, Role::Muted);
    let mut x = g.put(2, left, "TC", kind_role()) + 2;
    x = g.put(2, x, &form.name, Role::Title) + 2;
    g.put(2, x, &fit(&form.description, width.saturating_sub(x - left + 34)), Role::Text);
    let facts = format!("PUS {} · APID {} · {}", form.pus, form.apid, form.root_label);
    g.put(2, (left + width).saturating_sub(facts.chars().count()), &facts, Role::Muted);
    let mut y = 3;
    if let Some((text, bad)) = &form.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    y += 1;
    let slots = form.slots();
    let hidden = if form.fixed > 0 { format!("{} fixed, not asked", form.fixed) } else { String::new() };
    g.heading(y, left, width, &if hidden.is_empty() { format!("Arguments · {}", slots.len()) } else { format!("Arguments · {} · {hidden}", slots.len()) });
    y += 1;
    let (xn, xd, xv, xt, xc) = (left + 1, left + 22, left + 50, left + 74, left + 96);
    if slots.is_empty() {
        g.put(y, xn, "Nothing to fill: this telecommand takes no arguments.", Role::Muted);
        y += 1;
    }
    let mut anchor = y;
    for (i, slot) in slots.iter().enumerate() {
        let p = &form.params[slot.param];
        let indent = slot.path.len() * 2;
        let path = if slot.path.is_empty() { String::new() } else { format!("{} ", slot.path.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(".")) };
        g.put(y, xn + indent, &fit(&format!("{path}{}", p.tc.name), xd - xn - indent - 1), Role::Title);
        g.put(y, xd, &fit(&p.domain.description, xv - xd - 2), Role::Muted);
        let value = form.value(slot);
        let on = i == form.focus;
        let shown = if on && form.editing {
            format!("{value}▏")
        } else if !p.domain.aliases.is_empty() || p.group > 0 {
            format!("‹ {value} ›")
        } else if value.is_empty() {
            "—".to_string()
        } else {
            value.clone()
        };
        let end = g.put(y, xv, &fit_tail(&shown, xt - xv - 2), if on { Role::Title } else { Role::Text });
        if on && form.editing {
            g.panels.push((y, xv..end.max(xv + 12)));
        }
        let mut t = fenix_mib::types::decode(&p.domain.ptc, &p.domain.pfc).name;
        if !p.domain.unit.is_empty() {
            t = format!("{t} {}", p.domain.unit);
        }
        if p.group > 0 {
            t = format!("repeats next {}", p.group);
        }
        g.put(y, xt, &fit(&t, xc - xt - 2), Role::Muted);
        let warnings = form.warnings_for(slot);
        let (mark, role) = match warnings.first() {
            None => {
                let allowed = if !p.domain.ranges.is_empty() {
                    p.domain.ranges.iter().map(|(a, b)| format!("{a}..{b}")).collect::<Vec<_>>().join(", ")
                } else {
                    String::new()
                };
                (format!("✓ {allowed}"), Role::Good)
            }
            Some(w) => (format!("⚠ {}", w.split_once(": ").map(|(_, r)| r).unwrap_or(w)), Role::Warn),
        };
        g.put(y, xc, &fit(&mark, (left + width).saturating_sub(xc)), role);
        if on {
            g.focus(y, left..left + width);
            anchor = y;
        }
        y += 1;
    }
    y += 1;
    let warnings = form.warnings();
    let head = if warnings == 0 { "Command".to_string() } else { format!("Command · {warnings} warning{}", if warnings == 1 { "" } else { "s" }) };
    g.heading(y, left, width, &head);
    y += 1;
    let rendered = form.rendered();
    let lines = wrap(&rendered, width - 2);
    for line in if lines.is_empty() { vec![String::new()] } else { lines } {
        let e = g.put(y, left + 2, &line, Role::Title);
        g.panels.push((y, left + 1..e.max(left + 2) + 1));
        y += 1;
    }
    y += 1;
    if form.show_bytes {
        y += 1;
        match bytes {
            Some(Ok(b)) => {
                g.heading(y, left, width, &format!("Bytes · {} · sequence count {} · s next, S back to 0", b.len(), form.seq));
                y += 1;
                for (i, chunk) in b.chunks(16).enumerate() {
                    g.put(y, left + 2, &format!("{:04X}", i * 16), Role::Muted);
                    g.put(y, left + 8, &fenix_ccsds::field::hex(chunk), Role::Title);
                    y += 1;
                }
            }
            Some(Err(e)) => {
                g.heading(y, left, width, "Bytes");
                y += 1;
                g.put(y, left + 2, &fit(&format!("can't build the packet: {e}"), width - 2), Role::Bad);
                y += 1;
            }
            None => {}
        }
        y += 1;
    }
    g.put(y, left, "Inserted as text where the form was opened -- nothing is sent anywhere.", Role::Muted);

    if let Some(at) = form.menu {
        if let Some(slot) = slots.get(form.focus) {
            let p = &form.params[slot.param];
            let mut rows = vec![vec![(format!("{} · {}", p.tc.name, p.domain.description), Role::Title)], Vec::new()];
            for (i, a) in p.domain.aliases.iter().enumerate() {
                let n = if i < 9 { format!("{} ", i + 1) } else { "  ".into() };
                let on = i == at;
                rows.push(vec![(n, Role::Accent), (format!("{}{a}", if on { "› " } else { "  " }), if on { Role::Title } else { Role::Text })]);
            }
            rows.push(Vec::new());
            rows.push(vec![("Enter picks · Esc leaves it".into(), Role::Muted)]);
            g.popup = Some(Popup { line: anchor, col: xv, rows });
        }
    } else if form.help {
        let rows: Vec<Vec<(String, Role)>> = [
            ("j k Tab", "move between arguments"),
            ("Enter", "type a value, or pick a status text"),
            ("i c", "type, clear and type"),
            ("h l", "the previous, next status text"),
            ("+ -", "one more, one fewer repetition"),
            ("R", "the MIB's defaults again"),
            ("d", "the telecommand's page"),
            ("b s S", "the packet's bytes, next sequence count, back to 0"),
            ("y D", "copy the bytes for this file, decode them"),
            ("Ctrl-Enter I", "insert"),
            ("q Esc", "leave without inserting"),
        ]
        .iter()
        .map(|(k, w)| vec![(format!("{k:<14}"), Role::Accent), (w.to_string(), Role::Text)])
        .collect();
        let mut all = vec![vec![("Keys".to_string(), Role::Title)], Vec::new()];
        all.extend(rows);
        g.popup = Some(Popup { line: anchor, col: left + 4, rows: all });
    }
    let keys: &[(&str, &str)] = if form.editing {
        &[("Enter", "keep"), ("Tab", "next"), ("Ctrl-Enter", "insert")]
    } else if form.menu.is_some() {
        &[("Enter", "pick"), ("Esc", "leave it")]
    } else {
        &[("Enter", "edit"), ("h l", "change"), ("+ -", "repeat"), ("Ctrl-Enter", "insert"), ("b", "bytes"), ("y", "copy bytes"), ("D", "decode them"), ("R", "defaults"), ("d", "definition"), ("?", "all keys"), ("q", "cancel")]
    };
    g.keys(left, width, keys);
    g.finish()
}

/// A call `line` renders, read back into `(name, value)` arguments: each
/// argument template with the name filled in is looked for, and its
/// value runs to the template's text after `{value}`, else to the
/// separator.
pub fn read_arguments(line: &str, names: &[String], t: &Templates) -> Vec<(String, String)> {
    let (before, after) = t.argument.split_once("{value}").unwrap_or((t.argument.as_str(), ""));
    let sep = t.separator.trim();
    let mut found: Vec<(usize, String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        let prefix = before.replace("{name}", name);
        let suffix = after.replace("{name}", name);
        let mut from = 0;
        while let Some(i) = line[from..].find(&prefix) {
            let start = from + i + prefix.len();
            // The name mustn't just be the end of a longer one.
            let ok_before = from + i == 0 || !line[..from + i].ends_with(|c: char| c.is_alphanumeric() || c == '_');
            let rest = &line[start..];
            let end = if !suffix.is_empty() {
                rest.find(&suffix)
            } else {
                let stop = [sep, "]", ")", "}"].iter().filter(|s| !s.is_empty()).filter_map(|s| rest.find(s)).min();
                Some(stop.unwrap_or(rest.len()))
            };
            if let (true, Some(end)) = (ok_before, end) {
                found.push((from + i, name.clone(), rest[..end].trim().to_string()));
            }
            from = start;
        }
    }
    found.sort_by_key(|f| f.0);
    found.into_iter().map(|(_, n, v)| (n, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fenix_mib::Kind;

    fn templates() -> Templates {
        Templates { command: "tc::send {mnemo} {arguments}".into(), argument: "-{name} {value}".into(), separator: ", ".into() }
    }

    fn form_with(ccf: &str, cdf: &str, cpc: &str) -> (InsertForm, MibSet, fenix_mib::MibRoot) {
        let root = crate::mib_page::tests::fixture();
        std::fs::write(root.path.join("ccf.dat"), ccf).unwrap();
        std::fs::write(root.path.join("cdf.dat"), cdf).unwrap();
        if !cpc.is_empty() {
            std::fs::write(root.path.join("cpc.dat"), cpc).unwrap();
        }
        let set = MibSet::load(vec![root.clone()], None);
        let tc = DefRef { kind: Kind::Telecommand, index: 0 };
        (InsertForm::new(&set, MibKey { roots: vec![root.clone()], ..Default::default() }, tc, templates(), None, true), set, root)
    }

    #[test]
    fn defaults_fill_the_form_and_the_command_follows_the_values() {
        let (mut f, _set, root) = form_with(
            "ZTC08101\tSet heater control mode\t\t\tN\t\t8\t1\t1010\n",
            "ZTC08101\tF\tFunction\t8\t0\t0\tPTC00001\t\t12\t\nZTC08101\tE\tLine\t8\t8\t0\tPTH00101\t\t\t\nZTC08101\tE\tMode\t8\t16\t0\tPTH00102\t\t\t\n",
            "",
        );
        assert_eq!(f.rendered(), "tc::send ZTC08101 -PTH00101 , -PTH00102 AUTO");
        assert_eq!(f.warnings(), 1, "the line has no value yet");
        f.key(Key::Enter);
        f.key(Key::Char('3'));
        f.key(Key::Enter);
        f.key(Key::Char('j'));
        f.key(Key::Char('l'));
        assert_eq!(f.rendered(), "tc::send ZTC08101 -PTH00101 3, -PTH00102 OFF");
        assert_eq!(f.warnings(), 0);
        f.key(Key::Char('k'));
        f.key(Key::Char('c'));
        f.key(Key::Char('9'));
        f.key(Key::Escape);
        assert_eq!(f.warnings(), 1, "9 is outside 1..8");
        let text = layout(&f, None, 160).text;
        assert!(text.contains("outside"), "{text}");
        assert!(text.contains("1 FIXED, NOT ASKED"), "{text}");
        assert_eq!(f.key(Key::CtrlEnter), Action::Insert("tc::send ZTC08101 -PTH00101 9, -PTH00102 OFF".into()));
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_counter_repeats_the_group_after_it() {
        let (mut f, _set, root) = form_with(
            "ZTC11004\tInsert activities\t\t\tN\t\t11\t4\t1008\n",
            "ZTC11004\tE\tCount\t8\t0\t2\tPN\t\t\t\nZTC11004\tE\tTime\t32\t8\t0\tPT\t\t\t\nZTC11004\tE\tWhat\t16\t40\t0\tPW\t\t\t\nZTC11004\tE\tAfter\t8\t56\t0\tPA\t\t\t\n",
            "PN\tCount\t3\t4\t\t\t\t\t\t\t\t\t0\nPT\tTime\t3\t14\nPW\tWhat\t3\t12\nPA\tAfter\t3\t4\n",
        );
        assert_eq!(f.arguments().iter().map(|a| a.0.as_str()).collect::<Vec<_>>(), vec!["PN", "PA"]);
        f.key(Key::Char('+'));
        f.key(Key::Char('+'));
        let names: Vec<String> = f.arguments().into_iter().map(|a| a.0).collect();
        assert_eq!(names, vec!["PN", "PT", "PW", "PT", "PW", "PA"]);
        assert!(layout(&f, None, 160).text.contains("2 PW"), "the repetition is numbered");
        f.fill(&[("PN".into(), "1".into()), ("PT".into(), "100".into()), ("PW".into(), "7".into()), ("PA".into(), "5".into())]);
        assert_eq!(f.arguments(), vec![("PN".into(), "1".into()), ("PT".into(), "100".into()), ("PW".into(), "7".into()), ("PA".into(), "5".into())]);
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_rendered_call_reads_back_into_its_arguments() {
        let t = templates();
        let names: Vec<String> = ["PTH00101", "PTH00102"].iter().map(|s| s.to_string()).collect();
        let args = read_arguments("  tc::send ZTC08101 -PTH00101 3, -PTH00102 AUTO", &names, &t);
        assert_eq!(args, vec![("PTH00101".into(), "3".into()), ("PTH00102".into(), "AUTO".into())]);
        let t = Templates { command: "x ARGUMENTS=[{arguments}]".into(), argument: "{name}={value}".into(), separator: ", ".into() };
        let args = read_arguments("x ARGUMENTS=[PTH00101=3, XPTH00102=1, PTH00102=ON]", &names, &t);
        assert_eq!(args, vec![("PTH00101".into(), "3".into()), ("PTH00102".into(), "ON".into())]);
    }
}
