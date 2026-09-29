//! `SPC k k`: the MIB page. A tab for each kind of definition -- `1`
//! telecommands, `2` TC parameters, `3` TM packets, `4` TM parameters,
//! `5` calibrations -- each a list with the columns that kind needs,
//! narrowed by what's typed after `/` (words, and `field:value` filters,
//! see `fenix_mib::Query`), with a preview of the selected row when the
//! pane is wide enough. `Enter` opens the definition's own page
//! (`mib_def`), `i` the insert form for a telecommand. `!` lists what's
//! wrong in the MIBs' files. `f` opens the filters as a form, for when
//! you don't remember their names.
//!
//! `6` groups the telecommands and packets by PUS service: `]`/`[` go
//! from one service to the next, `z` folds them to one line each -- an
//! index to pick a service from.
//!
//! Pure like the other pages: the host loads the set and carries out the
//! `Action`s.

use std::path::PathBuf;

use fenix_mib::query::Hit;
use fenix_mib::{DefRef, Kind, MibSet, Query};
use fenix_project::ProjectKind;

use crate::mib_def;
use crate::page::{edit_line, fit, fit_tail, frame, insert_at, with_caret, wrap, Grid, Key, Page, Popup, Role};

/// What the page reads besides its own state.
pub struct Ctx<'a> {
    pub set: Option<&'a MibSet>,
    /// The set is being read.
    pub loading: bool,
    pub apid_hex: bool,
    /// Where the MIBs come from: "mission-c's settings", "your settings".
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Open(DefRef),
    Insert(DefRef),
    Copy(String),
    Reload,
    /// The MIB settings: this project's (`true`), or yours.
    Settings(bool),
    OpenFile(PathBuf, usize),
}

/// Which MIBs a page shows, the one that wins a name several define, and
/// the project they're for (whose templates an insert uses): the loaded
/// set it's looked up by.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct MibKey {
    pub roots: Vec<fenix_mib::MibRoot>,
    pub default: Option<String>,
    pub project: Option<std::path::PathBuf>,
}

/// How many rows the list lays out around the selected one: enough to
/// scroll through, few enough that a 6,000-parameter MIB stays quick.
const WINDOW: usize = 300;

pub struct MibPage {
    /// The MIBs it shows: the set it's looked up by.
    pub key: MibKey,
    pub tab: Kind,
    query: String,
    searching: bool,
    /// The selected row, per tab.
    sel: [usize; 5],
    /// The column the list is sorted by; `None`: best match, then name.
    sort: Option<usize>,
    /// Only this MIB's definitions.
    only: Option<usize>,
    preview: bool,
    help: bool,
    /// The services view (tab 6), and its selected definition.
    pub services: bool,
    svc_sel: usize,
    /// The services folded to a line each, and the selected one.
    svc_folded: bool,
    svc_head: usize,
    /// `f`: the filters as a form.
    form: Option<FilterForm>,
    /// The problems list, and its selected row.
    problems: Option<usize>,
    pub note: Option<(String, bool)>,
}

fn slot(kind: Kind) -> usize {
    Kind::ALL.iter().position(|&k| k == kind).unwrap_or(0)
}

/// `1234567` as `1,234,567`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

impl MibPage {
    pub fn new(key: MibKey) -> Self {
        MibPage {
            key,
            tab: Kind::Telecommand,
            query: String::new(),
            searching: false,
            sel: [0; 5],
            sort: None,
            only: None,
            preview: true,
            help: false,
            services: false,
            svc_sel: 0,
            svc_folded: false,
            svc_head: 0,
            form: None,
            problems: None,
            note: None,
        }
    }

    pub fn typing(&self) -> bool {
        self.searching || self.form.is_some()
    }

    pub fn claims_space(&self) -> bool {
        self.typing()
    }

    pub fn paste(&mut self, text: &str) {
        if let Some(form) = &mut self.form {
            let field = &mut form.fields[form.at];
            insert_at(&mut field.value, &mut form.caret, text);
            self.query = form.query();
        } else if self.searching {
            self.query.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    /// The rows of `kind`, as the search, the MIB filter and the sort
    /// leave them.
    pub fn hits(&self, set: &MibSet, kind: Kind) -> Vec<Hit> {
        let mut q = Query::parse(&self.query);
        if let Some(only) = self.only {
            q.filters.push(("mib".into(), set.root_label(only).to_string()));
        }
        let mut hits = if q.is_empty() {
            let mut all: Vec<Hit> =
                (0..set.entries(kind).len()).map(|index| Hit { def: DefRef { kind, index }, score: 0, matched_on: None }).collect();
            all.sort_by(|a, b| set.get(a.def).name.cmp(&set.get(b.def).name));
            all
        } else {
            q.search(set, kind)
        };
        if let Some(col) = self.sort {
            let key = |h: &Hit| {
                let e = set.get(h.def);
                match col {
                    0 => e.name.clone(),
                    c if c <= e.cols.len() => e.cols[c - 1].clone(),
                    _ => e.description.to_lowercase(),
                }
            };
            hits.sort_by(|a, b| natural(&key(a), &key(b)).then_with(|| set.get(a.def).name.cmp(&set.get(b.def).name)));
        }
        hits
    }

    /// The Services tab as the search leaves it: the telecommands and
    /// packets the search finds, or every one of a service whose name has
    /// the words typed ("housekeeping").
    pub fn services_shown(&self, set: &MibSet) -> Vec<(u8, Vec<DefRef>)> {
        let all = services(set, self.only);
        if self.query.trim().is_empty() {
            return all;
        }
        let found: std::collections::HashSet<DefRef> = [Kind::Telecommand, Kind::TmPacket].into_iter().flat_map(|k| self.hits(set, k)).map(|h| h.def).collect();
        let words: Vec<String> = self.query.split_whitespace().filter(|w| !w.contains(':')).map(str::to_lowercase).collect();
        all.into_iter()
            .filter_map(|(service, defs)| {
                let name = fenix_ccsds::pus::service_name(service).unwrap_or("").to_lowercase();
                if !words.is_empty() && words.iter().all(|w| name.contains(w.as_str()) || service.to_string() == *w) {
                    return Some((service, defs));
                }
                let defs: Vec<DefRef> = defs.into_iter().filter(|d| found.contains(d)).collect();
                (!defs.is_empty()).then_some((service, defs))
            })
            .collect()
    }

    /// The problems with every word of the search in their MIB, file or
    /// message.
    pub fn problems_shown<'a>(&self, set: &'a MibSet) -> Vec<&'a fenix_mib::Problem> {
        let words: Vec<String> = self.query.split_whitespace().map(str::to_lowercase).collect();
        set.problems()
            .iter()
            .filter(|p| {
                let hay = format!("{} {}:{} {}", set.root_label(p.root), p.file.display(), p.line.unwrap_or(0), p.message).to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .collect()
    }

    fn selected(&self, set: &MibSet) -> Option<DefRef> {
        let hits = self.hits(set, self.tab);
        hits.get(self.sel[slot(self.tab)].min(hits.len().saturating_sub(1))).map(|h| h.def)
    }

    fn move_to(&mut self, set: &MibSet, row: isize) {
        let n = self.hits(set, self.tab).len();
        let s = &mut self.sel[slot(self.tab)];
        *s = row.clamp(0, n.saturating_sub(1) as isize) as usize;
    }

    pub fn key(&mut self, key: Key, ctx: &Ctx) -> Action {
        self.note = None;
        if self.help {
            self.help = false;
            return Action::None;
        }
        if self.form.is_some() {
            self.form_key(key);
            return Action::None;
        }
        if self.searching {
            match key {
                Key::Escape => {
                    self.searching = false;
                    self.query.clear();
                }
                Key::Enter | Key::Down => self.searching = false,
                Key::Backspace => {
                    self.query.pop();
                }
                Key::Tab => self.complete_field(),
                Key::Char(c) => self.query.push(c),
                Key::Space => self.query.push(' '),
                _ => {}
            }
            self.sel = [0; 5];
            self.svc_sel = 0;
            if self.problems.is_some() {
                self.problems = Some(0);
            }
            return Action::None;
        }
        let Some(set) = ctx.set else {
            return match key {
                Key::Char('q') | Key::Escape => Action::Close,
                Key::Char('a') => Action::Settings(true),
                Key::Char('A') => Action::Settings(false),
                Key::Char('R') => Action::Reload,
                _ => Action::None,
            };
        };
        if let Some(at) = self.problems {
            let shown = self.problems_shown(set);
            let n = shown.len();
            match key {
                Key::Char('/') => self.searching = true,
                Key::Escape | Key::Char('!') | Key::Char('q') => self.problems = None,
                Key::Down | Key::Char('j') => self.problems = Some((at + 1).min(n.saturating_sub(1))),
                Key::Up | Key::Char('k') => self.problems = Some(at.saturating_sub(1)),
                Key::Char('g') => self.problems = Some(0),
                Key::Char('G') => self.problems = Some(n.saturating_sub(1)),
                Key::Enter => {
                    if let Some(p) = shown.get(at) {
                        return Action::OpenFile(p.file.clone(), p.line.unwrap_or(1));
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        if self.services {
            let shown = self.services_shown(set);
            if self.svc_folded {
                return self.folded_key(key, set, &shown);
            }
            let defs: Vec<DefRef> = shown.iter().flat_map(|(_, d)| d.iter().copied()).collect();
            let starts = service_starts(&shown);
            let current = starts.iter().rposition(|&s| s <= self.svc_sel).unwrap_or(0);
            match key {
                Key::Char('/') => {
                    self.searching = true;
                    self.svc_sel = 0;
                }
                Key::Char('f') => self.open_form(set),
                Key::Escape if !self.query.is_empty() => {
                    self.query.clear();
                    self.svc_sel = 0;
                }
                Key::Char(']') => {
                    if let Some(&next) = starts.get(current + 1) {
                        self.svc_sel = next;
                    }
                }
                Key::Char('[') => {
                    // To the top of this service first, then the one before.
                    let here = starts.get(current).copied().unwrap_or(0);
                    self.svc_sel = if self.svc_sel > here { here } else { starts.get(current.saturating_sub(1)).copied().unwrap_or(0) };
                }
                Key::Char('z') => {
                    self.svc_folded = true;
                    self.svc_head = current;
                }
                Key::Char('d') => self.svc_sel = (self.svc_sel + 20).min(defs.len().saturating_sub(1)),
                Key::Char('u') => self.svc_sel = self.svc_sel.saturating_sub(20),
                Key::Char('q') | Key::Escape => return Action::Close,
                Key::Char(c @ '1'..='5') => {
                    self.services = false;
                    self.tab = Kind::ALL[c as usize - '1' as usize];
                }
                Key::Tab => {
                    self.services = false;
                    self.tab = Kind::Telecommand;
                }
                Key::BackTab => {
                    self.services = false;
                    self.tab = Kind::Calibration;
                }
                Key::Down | Key::Char('j') => self.svc_sel = (self.svc_sel + 1).min(defs.len().saturating_sub(1)),
                Key::Up | Key::Char('k') => self.svc_sel = self.svc_sel.saturating_sub(1),
                Key::Char('g') => self.svc_sel = 0,
                Key::Char('G') => self.svc_sel = defs.len().saturating_sub(1),
                Key::Enter | Key::Char('l') => {
                    if let Some(d) = defs.get(self.svc_sel) {
                        return Action::Open(*d);
                    }
                }
                Key::Char('i') => match defs.get(self.svc_sel) {
                    Some(d) if d.kind == Kind::Telecommand => return Action::Insert(*d),
                    _ => {}
                },
                Key::Char('m') => {
                    let n = set.roots().len();
                    self.only = match self.only {
                        _ if n < 2 => None,
                        None => Some(0),
                        Some(i) if i + 1 < n => Some(i + 1),
                        Some(_) => None,
                    };
                    self.svc_sel = 0;
                }
                Key::Char('?') => self.help = true,
                _ => {}
            }
            return Action::None;
        }
        let row = self.sel[slot(self.tab)] as isize;
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('?') => self.help = true,
            Key::Char(c @ '1'..='5') => self.tab = Kind::ALL[c as usize - '1' as usize],
            Key::Char('6') => {
                self.services = true;
                self.problems = None;
            }
            Key::Tab if self.tab == Kind::Calibration => self.services = true,
            Key::Tab => self.tab = Kind::ALL[(slot(self.tab) + 1) % 5],
            Key::BackTab => self.tab = Kind::ALL[(slot(self.tab) + 4) % 5],
            Key::Char('/') => {
                self.searching = true;
                self.sel = [0; 5];
            }
            Key::Char('f') => self.open_form(set),
            Key::Down | Key::Char('j') => self.move_to(set, row + 1),
            Key::Up | Key::Char('k') => self.move_to(set, row - 1),
            Key::Char('d') => self.move_to(set, row + 20),
            Key::Char('u') => self.move_to(set, row - 20),
            Key::Char('g') => self.move_to(set, 0),
            Key::Char('G') => self.move_to(set, isize::MAX / 2),
            Key::Char('o') => {
                let n = self.tab.headers().len() + 1;
                self.sort = match self.sort {
                    None => Some(0),
                    Some(c) if c + 1 < n => Some(c + 1),
                    Some(_) => None,
                };
                self.move_to(set, 0);
            }
            Key::Char('m') => {
                let n = set.roots().len();
                self.only = match self.only {
                    _ if n < 2 => None,
                    None => Some(0),
                    Some(i) if i + 1 < n => Some(i + 1),
                    Some(_) => None,
                };
                self.sel = [0; 5];
            }
            Key::Char('p') => self.preview = !self.preview,
            Key::Char('!') => {
                if set.problems().is_empty() {
                    self.note = Some(("no problems in these MIBs".into(), false));
                } else {
                    self.problems = Some(0);
                }
            }
            Key::Char('R') => return Action::Reload,
            Key::Char('a') => return Action::Settings(true),
            Key::Char('A') => return Action::Settings(false),
            Key::Enter | Key::Char('l') => {
                if let Some(def) = self.selected(set) {
                    return Action::Open(def);
                }
            }
            Key::Char('i') => match self.selected(set) {
                Some(def) if def.kind == Kind::Telecommand => return Action::Insert(def),
                Some(_) => self.note = Some(("i inserts a telecommand -- this tab has none".into(), true)),
                None => {}
            },
            Key::Char('y') => {
                if let Some(def) = self.selected(set) {
                    return Action::Copy(set.get(def).name.clone());
                }
            }
            _ => {}
        }
        Action::None
    }

    /// A key with the services folded: one row per service.
    fn folded_key(&mut self, key: Key, set: &MibSet, shown: &[(u8, Vec<DefRef>)]) -> Action {
        let n = shown.len();
        match key {
            Key::Down | Key::Char('j') | Key::Char(']') => self.svc_head = (self.svc_head + 1).min(n.saturating_sub(1)),
            Key::Up | Key::Char('k') | Key::Char('[') => self.svc_head = self.svc_head.saturating_sub(1),
            Key::Char('g') => self.svc_head = 0,
            Key::Char('G') => self.svc_head = n.saturating_sub(1),
            // Unfolded, on the service picked.
            Key::Enter | Key::Char('l') | Key::Char('z') => {
                self.svc_folded = false;
                self.svc_sel = service_starts(shown).get(self.svc_head).copied().unwrap_or(0);
            }
            Key::Char('/') => {
                self.searching = true;
                self.svc_head = 0;
            }
            Key::Char('f') => self.open_form(set),
            Key::Escape if !self.query.is_empty() => {
                self.query.clear();
                self.svc_head = 0;
            }
            Key::Escape | Key::Char('q') => return Action::Close,
            Key::Char(c @ '1'..='5') => {
                self.services = false;
                self.tab = Kind::ALL[c as usize - '1' as usize];
            }
            Key::Char('?') => self.help = true,
            _ => {}
        }
        Action::None
    }

    /// `f`: the filters for what's shown, as a form filled from the
    /// search.
    fn open_form(&mut self, set: &MibSet) {
        let fields = form_fields(if self.services { None } else { Some(self.tab) }, set.roots().len() > 1);
        self.form = Some(FilterForm::new(fields, &self.query));
    }

    /// A key in the filter form. Every change narrows the list behind it
    /// straight away; `Esc` puts the search back as it was.
    fn form_key(&mut self, key: Key) {
        let Some(form) = &mut self.form else { return };
        let n = form.fields.len();
        match key {
            Key::Escape => {
                self.query = form.before.clone();
                self.form = None;
            }
            Key::Enter => self.form = None,
            Key::Tab | Key::Down => form.move_to((form.at + 1) % n),
            Key::BackTab | Key::Up => form.move_to((form.at + n - 1) % n),
            // Every field emptied.
            Key::CtrlC => {
                for field in &mut form.fields {
                    field.value.clear();
                }
                form.caret = 0;
            }
            key => {
                let field = &mut form.fields[form.at];
                edit_line(&mut field.value, &mut form.caret, key);
            }
        }
        if let Some(form) = &self.form {
            self.query = form.query();
        }
        self.sel = [0; 5];
        self.svc_sel = 0;
        self.svc_head = 0;
    }

    /// `Tab` in the search: completes the field name being typed.
    fn complete_field(&mut self) {
        let start = self.query.rfind(' ').map(|i| i + 1).unwrap_or(0);
        let word = self.query[start..].to_ascii_lowercase();
        if word.is_empty() || word.contains(':') {
            return;
        }
        if let Some(name) = fenix_mib::query::field_names().into_iter().find(|n| n.starts_with(&word)) {
            self.query.truncate(start);
            self.query.push_str(name);
            self.query.push(':');
        }
    }
}

/// The MIB by PUS service: each service type used, with its
/// telecommands, then its TM packets, by subtype.
pub fn services(set: &MibSet, only: Option<usize>) -> Vec<(u8, Vec<DefRef>)> {
    let mut by: std::collections::BTreeMap<u8, Vec<(u8, u8, String, DefRef)>> = std::collections::BTreeMap::new();
    for kind in [Kind::Telecommand, Kind::TmPacket] {
        let (tf, sf) = if kind == Kind::Telecommand { ("CCF_TYPE", "CCF_STYPE") } else { ("PID_TYPE", "PID_STYPE") };
        for (index, e) in set.entries(kind).iter().enumerate() {
            if only.is_some_and(|o| o != e.root) {
                continue;
            }
            let (Some(t), Some(st)) = (fenix_mib::types::parse_int(e.row.clean(tf)), fenix_mib::types::parse_int(e.row.clean(sf))) else { continue };
            if !(0..=255).contains(&t) || !(0..=255).contains(&st) {
                continue;
            }
            let order = if kind == Kind::Telecommand { 0 } else { 1 };
            by.entry(t as u8).or_default().push((order, st as u8, e.name.clone(), DefRef { kind, index }));
        }
    }
    by.into_iter()
        .map(|(s, mut v)| {
            v.sort();
            (s, v.into_iter().map(|x| x.3).collect())
        })
        .collect()
}

/// Where each service's rows start in the services view's list.
fn service_starts(shown: &[(u8, Vec<DefRef>)]) -> Vec<usize> {
    let mut at = 0;
    shown
        .iter()
        .map(|(_, defs)| {
            let start = at;
            at += defs.len();
            start
        })
        .collect()
}

/// One field of the filter form: what it's called, the filter it sets
/// (`""`: the free words), and what's typed in it.
struct FormField {
    label: &'static str,
    key: &'static str,
    value: String,
}

/// `f`: the search's `field:value` filters as a form.
struct FilterForm {
    fields: Vec<FormField>,
    at: usize,
    caret: usize,
    /// The search as it was, for `Esc`.
    before: String,
}

impl FilterForm {
    /// The form for `fields`, filled from `query`: its filters in their
    /// fields, everything else -- words, filters the form has no field
    /// for -- in the first one.
    fn new(fields: Vec<(&'static str, &'static str)>, query: &str) -> Self {
        let mut fields: Vec<FormField> = fields.into_iter().map(|(label, key)| FormField { label, key, value: String::new() }).collect();
        let mut rest = Vec::new();
        for token in query.split_whitespace() {
            let field = token.split_once(':').and_then(|(k, v)| {
                let k = k.to_ascii_lowercase();
                let k = if k == "subtype" { "stype".to_string() } else { k };
                fields.iter().position(|f| !f.key.is_empty() && f.key == k && !v.is_empty()).map(|i| (i, v))
            });
            match field {
                Some((i, v)) => fields[i].value = v.to_string(),
                None => rest.push(token),
            }
        }
        fields[0].value = rest.join(" ");
        // Straight on the first filter when nothing's set yet: words are
        // what `/` is for.
        let at = if fields.iter().any(|f| !f.value.is_empty()) { 0 } else { 1.min(fields.len() - 1) };
        let caret = fields[at].value.chars().count();
        FilterForm { fields, at, caret, before: query.to_string() }
    }

    fn move_to(&mut self, at: usize) {
        self.at = at;
        self.caret = self.fields[at].value.chars().count();
    }

    /// The search the form stands for.
    fn query(&self) -> String {
        let mut parts = Vec::new();
        for field in &self.fields {
            // A filter's value is one word: spaces typed in it are dropped.
            let value: String = if field.key.is_empty() { field.value.trim().to_string() } else { field.value.split_whitespace().collect() };
            if value.is_empty() {
                continue;
            }
            parts.push(if field.key.is_empty() { value } else { format!("{}:{value}", field.key) });
        }
        parts.join(" ")
    }
}

/// The form's fields for a tab (`None`: the services), as (label, filter).
fn form_fields(tab: Option<Kind>, several_mibs: bool) -> Vec<(&'static str, &'static str)> {
    let mut fields = vec![("Name or text", "")];
    fields.extend_from_slice(match tab {
        None => &[("Service", "type"), ("Subtype", "stype"), ("APID", "apid"), ("Subsystem", "subsys"), ("SPID", "spid")][..],
        Some(Kind::Telecommand) => &[("Service", "type"), ("Subtype", "stype"), ("APID", "apid"), ("Subsystem", "subsys"), ("Critical", "critical")][..],
        Some(Kind::TmPacket) => &[("Service", "type"), ("Subtype", "stype"), ("APID", "apid"), ("SPID", "spid")][..],
        Some(Kind::TcParam) => &[("PTC", "ptc"), ("PFC", "pfc"), ("Unit", "unit"), ("Calibration", "cal")][..],
        Some(Kind::TmParam) => &[("PTC", "ptc"), ("PFC", "pfc"), ("Unit", "unit"), ("Subsystem", "subsys"), ("Calibration", "cal")][..],
        Some(Kind::Calibration) => &[("Kind", "cal"), ("Unit", "unit")][..],
    });
    if several_mibs {
        fields.push(("MIB", "mib"));
    }
    fields
}

/// Compares numbers as numbers and the rest as text.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    match (fenix_mib::types::parse_int(a), fenix_mib::types::parse_int(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// A cell of an entry's row, as the list shows it.
pub fn cell(set: &MibSet, def: DefRef, col: usize, apid_hex: bool) -> String {
    let e = set.get(def);
    if col == 0 {
        return e.name.clone();
    }
    let text = e.cols.get(col - 1).cloned().unwrap_or_default();
    if def.kind.headers().get(col) == Some(&"APID") {
        fenix_mib::types::apid(&text, apid_hex)
    } else {
        text
    }
}

pub fn layout(page: &MibPage, ctx: &Ctx, cols: usize) -> Page {
    let (left, width) = frame(cols, 190);
    let mut g = Grid::new();
    g.put(1, left, "MIB", Role::Title);
    let mut x = left + 5;
    if let Some(set) = ctx.set {
        for (i, root) in set.roots().iter().enumerate() {
            let version = set.root_info(i).version.map(|v| format!(" · {v}")).unwrap_or_default();
            let text = format!("{}{version}", root.label);
            let on = page.only.is_none_or(|o| o == i);
            let role = if page.only == Some(i) { Role::Title } else if on { Role::Text } else { Role::Muted };
            let end = g.put(1, x, &text, role);
            if page.only == Some(i) {
                g.panels.push((1, x..end));
            }
            x = end + 3;
        }
    }
    let status = match (ctx.set, ctx.loading) {
        (_, true) => ("reading…".to_string(), Role::Muted),
        (Some(set), false) if !set.problems().is_empty() => {
            let n = set.problems().len();
            (format!("{n} problem{} · !", if n == 1 { "" } else { "s" }), Role::Warn)
        }
        (Some(_), false) => (ctx.source.clone(), Role::Muted),
        (None, false) => (String::new(), Role::Muted),
    };
    g.put(1, (left + width).saturating_sub(status.0.chars().count()), &status.0, status.1);

    let Some(set) = ctx.set else {
        empty(&mut g, ctx, left, width);
        g.keys(left, width, &[("a", "add to this project"), ("A", "add to yours"), ("R", "read again"), ("q", "close")]);
        return g.finish();
    };

    let counts: Vec<usize> = Kind::ALL.iter().map(|&k| page.hits(set, k).len()).collect();
    let search = if page.searching {
        format!("/ {}▏", page.query)
    } else if page.query.is_empty() {
        format!("/ search {} {}", thousands(counts[slot(page.tab)]), page.tab.plural().to_lowercase())
    } else {
        format!("/ {}  (Esc clears)", page.query)
    };
    let end = g.put(2, left, &fit_tail(&search, width), if page.searching { Role::Title } else { Role::Muted });
    if page.searching {
        g.panels.push((2, left..end.max(left + 40)));
    }
    let mut x = left;
    for (i, kind) in Kind::ALL.iter().enumerate() {
        let on = *kind == page.tab && !page.services;
        x = g.put(4, x, &(i + 1).to_string(), Role::Accent) + 1;
        let end = g.put(4, x, kind.plural(), if on { Role::Title } else { Role::Muted });
        if on {
            g.panels.push((4, x..end));
        }
        x = g.put(4, end + 1, &thousands(counts[i]), Role::Muted) + 3;
    }
    let svc = page.services_shown(set);
    x = g.put(4, x, "6", Role::Accent) + 1;
    let end = g.put(4, x, "Services", if page.services { Role::Title } else { Role::Muted });
    if page.services {
        g.panels.push((4, x..end));
    }
    g.put(4, end + 1, &svc.len().to_string(), Role::Muted);
    let sort = match page.sort {
        None if page.query.is_empty() => "by name".to_string(),
        None => "best match first".to_string(),
        Some(c) => format!("by {}", page.tab.headers().get(c).copied().unwrap_or("description").to_lowercase()),
    };
    let sort = format!("{sort} · o");
    g.put(4, (left + width).saturating_sub(sort.chars().count()), &sort, Role::Muted);
    let mut y = 5;
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;

    if page.services {
        let anchor = if page.svc_folded { services_folded(&mut g, page, set, &svc, left, width, y) } else { services_list(&mut g, page, set, &svc, ctx, left, width, y) };
        if svc.is_empty() {
            let why = if page.query.trim().is_empty() { "No telecommands or packets with a service type." } else { "Nothing matches -- Esc clears the search." };
            g.put(y, left, why, Role::Muted);
        }
        if page.help {
            help(&mut g, anchor, left);
        }
        form_popup(&mut g, page, left);
        let keys: &[(&str, &str)] = if page.form.is_some() {
            &[("Tab", "next field"), ("Enter", "keep"), ("Ctrl-C", "clear all"), ("Esc", "cancel")]
        } else if page.searching {
            &[("Enter", "keep"), ("Tab", "complete field"), ("Esc", "clear")]
        } else if page.svc_folded {
            &[("Enter", "open the service"), ("j k", "move"), ("/", "search"), ("f", "filter"), ("z", "unfold"), ("q", "close")]
        } else {
            &[("] [", "next, previous service"), ("z", "fold"), ("j k", "move"), ("/", "search"), ("f", "filter"), ("Enter", "open"), ("i", "insert"), ("m", "one MIB"), ("q", "close")]
        };
        g.keys(left, width, keys);
        return g.finish();
    }

    if let Some(at) = page.problems {
        problems(&mut g, set, &page.problems_shown(set), at, left, width, y);
        g.keys(left, width, &[("Enter", "open the line"), ("j k", "move"), ("/", "search"), ("Esc", "back")]);
        return g.finish();
    }

    let preview_w = if page.preview && width >= 120 { (width * 2 / 5).min(80) } else { 0 };
    let list_w = width - if preview_w > 0 { preview_w + 3 } else { 0 };
    let hits = page.hits(set, page.tab);
    let sel = page.sel[slot(page.tab)].min(hits.len().saturating_sub(1));
    let start = sel.saturating_sub(WINDOW / 2).min(hits.len().saturating_sub(WINDOW));
    let shown = &hits[start..(start + WINDOW).min(hits.len())];

    // Column widths from what's shown, within limits.
    let headers = page.tab.headers();
    let caps = [16usize, 20, 10, 10];
    let widths: Vec<usize> = (0..headers.len())
        .map(|c| shown.iter().map(|h| cell(set, h.def, c, ctx.apid_hex).chars().count()).max().unwrap_or(0).max(headers[c].len()).min(caps[c]))
        .collect();
    let mut x = left + 1;
    let mut xs = Vec::new();
    for (c, w) in widths.iter().enumerate() {
        xs.push(x);
        let label = if page.sort == Some(c) { format!("{} ↓", headers[c]) } else { headers[c].to_string() };
        g.put(y, x, &label.to_uppercase(), if page.sort == Some(c) { Role::Text } else { Role::Muted });
        x += w + 2;
    }
    let dx = x;
    let dlabel = if page.sort == Some(headers.len()) { "DESCRIPTION ↓" } else { "DESCRIPTION" };
    g.put(y, dx, dlabel, Role::Muted);
    y += 1;
    g.rule(y, left..left + list_w);
    y += 1;
    let top = y;
    if hits.is_empty() {
        let why = if page.query.is_empty() && page.only.is_none() {
            format!("No {} in these MIBs.", page.tab.plural().to_lowercase())
        } else {
            "Nothing matches -- Esc clears the search.".to_string()
        };
        g.put(y, left + 1, &why, Role::Muted);
    }
    if start > 0 {
        g.put(y, left + 1, &format!("↑ {} more", thousands(start)), Role::Muted);
        y += 1;
    }
    let mut anchor = top;
    for (i, hit) in shown.iter().enumerate() {
        let def = hit.def;
        for (c, w) in widths.iter().enumerate() {
            let role = if c == 0 { Role::Title } else { Role::Muted };
            g.put(y, xs[c], &fit(&cell(set, def, c, ctx.apid_hex), *w), role);
        }
        let e = set.get(def);
        let mut desc = e.description.clone();
        if let Some(on) = &hit.matched_on {
            desc = format!("{desc}  · {on}");
        }
        if set.roots().len() > 1 && e.root != set.default_root() {
            desc = format!("[{}] {desc}", set.root_label(e.root));
        }
        g.put(y, dx, &fit(&desc, (left + list_w).saturating_sub(dx)), Role::Text);
        if start + i == sel {
            g.focus(y, left..left + list_w);
            anchor = y;
        }
        y += 1;
    }
    let rest = hits.len().saturating_sub(start + shown.len());
    if rest > 0 {
        g.put(y, left + 1, &format!("↓ {} more -- / narrows the list", thousands(rest)), Role::Muted);
    }

    if preview_w > 0 {
        if let Some(hit) = hits.get(sel) {
            let px = left + width - preview_w;
            for (y, (text, role)) in (top.saturating_sub(2)..).zip(mib_def::preview(set, hit.def, ctx.apid_hex, preview_w)) {
                g.put(y, px, &fit(&text, preview_w), role);
            }
        }
    }

    if page.help {
        help(&mut g, anchor, left);
    }
    form_popup(&mut g, page, left);
    let insert = if page.tab == Kind::Telecommand { ("i", "insert") } else { ("", "") };
    let mut keys = vec![("1-5", "tabs"), ("/", "search"), ("f", "filter"), ("Enter", "open")];
    if !insert.0.is_empty() {
        keys.push(insert);
    }
    keys.extend([("o", "sort"), ("y", "copy name")]);
    if set.roots().len() > 1 {
        keys.push(("m", "one MIB"));
    }
    keys.extend([("p", "preview"), ("?", "all keys"), ("q", "close")]);
    if page.searching {
        keys = vec![("Enter", "keep"), ("Tab", "complete field"), ("Esc", "clear")];
    }
    if page.form.is_some() {
        keys = vec![("Tab", "next field"), ("Enter", "keep"), ("Ctrl-C", "clear all"), ("Esc", "cancel")];
    }
    g.keys(left, width, &keys);
    g.finish()
}

/// The services, a line each: the index `z` folds them into.
fn services_folded(g: &mut Grid, page: &MibPage, set: &MibSet, svc: &[(u8, Vec<DefRef>)], left: usize, width: usize, top: usize) -> usize {
    let mut y = top;
    let at = page.svc_head.min(svc.len().saturating_sub(1));
    for (i, (service, defs)) in svc.iter().enumerate() {
        let tcs = defs.iter().filter(|d| d.kind == Kind::Telecommand).count();
        let x = g.put(y, left + 1, &format!("{service:>3}"), Role::Accent) + 2;
        let x = g.put(y, x, &fit(service_label(*service), 34), Role::Title).max(left + 42);
        let counts = format!("{tcs:>4} TC  {:>4} TM", defs.len() - tcs);
        let x = g.put(y, x, &counts, Role::Muted) + 3;
        // The APIDs it's on, so "which service is on this APID" reads
        // off the index.
        let mut apids: Vec<String> = Vec::new();
        for d in defs {
            let e = set.get(*d);
            let col = if d.kind == Kind::Telecommand { "CCF_APID" } else { "PID_APID" };
            let a = fenix_mib::types::apid(e.row.clean(col), true);
            if !a.is_empty() && !apids.contains(&a) {
                apids.push(a);
            }
        }
        apids.sort_by(|a, b| natural(a, b));
        let apids = match apids.len() {
            0 => String::new(),
            1..=6 => format!("APID {}", apids.join(" ")),
            n => format!("APID {} … ({n})", apids[..5].join(" ")),
        };
        g.put(y, x, &fit(&apids, (left + width).saturating_sub(x)), Role::Muted);
        if i == at {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
    top + at
}

/// What a service is called in the list.
fn service_label(service: u8) -> &'static str {
    fenix_ccsds::pus::service_name(service).unwrap_or(if service >= 128 { "mission specific" } else { "not a standard service" })
}

/// A row of the services list: a service's heading, one of its
/// definitions (its index among all shown), or the gap after a service.
enum SvcRow {
    Head(u8, usize, usize),
    Def(usize, DefRef),
    Gap,
}

/// The services with their telecommands and packets -- only the rows
/// around the selected one are laid out, so a MIB with thousands stays
/// quick to move through.
#[allow(clippy::too_many_arguments)]
fn services_list(g: &mut Grid, page: &MibPage, set: &MibSet, svc: &[(u8, Vec<DefRef>)], ctx: &Ctx, left: usize, width: usize, top: usize) -> usize {
    let mut rows = Vec::new();
    let mut k = 0;
    for (service, defs) in svc {
        let tcs = defs.iter().filter(|d| d.kind == Kind::Telecommand).count();
        rows.push(SvcRow::Head(*service, tcs, defs.len() - tcs));
        for d in defs {
            rows.push(SvcRow::Def(k, *d));
            k += 1;
        }
        rows.push(SvcRow::Gap);
    }
    let sel = page.svc_sel.min(k.saturating_sub(1));
    let at = rows.iter().position(|r| matches!(r, SvcRow::Def(i, _) if *i == sel)).unwrap_or(0);
    let start = at.saturating_sub(WINDOW / 2).min(rows.len().saturating_sub(WINDOW));
    let mut y = top;
    if start > 0 {
        let above = rows[..start].iter().filter(|r| matches!(r, SvcRow::Def(..))).count();
        g.put(y, left, &format!("↑ {} more", thousands(above)), Role::Muted);
        y += 1;
    }
    // Starting inside a service: its heading still comes first.
    let open = rows[..start].iter().rposition(|r| matches!(r, SvcRow::Head(..))).filter(|_| !matches!(rows.get(start), Some(SvcRow::Head(..))));
    let mut anchor = top;
    let end = (start + WINDOW).min(rows.len());
    for row in open.map(|h| &rows[h]).into_iter().chain(&rows[start..end]) {
        match row {
            SvcRow::Head(service, tcs, tms) => {
                g.heading(y, left, width, &format!("{service} · {} · {tcs} TC · {tms} TM", service_label(*service)));
            }
            SvcRow::Gap => {}
            SvcRow::Def(i, d) => {
                let e = set.get(*d);
                let r = &e.row;
                let service = if d.kind == Kind::Telecommand { r.clean("CCF_TYPE") } else { r.clean("PID_TYPE") };
                let (st, apid) = if d.kind == Kind::Telecommand { (r.clean("CCF_STYPE"), r.clean("CCF_APID")) } else { (r.clean("PID_STYPE"), r.clean("PID_APID")) };
                let sub = match (fenix_mib::types::parse_int(service), fenix_mib::types::parse_int(st)) {
                    (Some(t), Some(n)) => fenix_ccsds::pus::subtype_name(t as u8, n as u8).unwrap_or(""),
                    _ => "",
                };
                let mut x = g.put(y, left + 1, if d.kind == Kind::Telecommand { "TC " } else { "TM " }, kind_role()) + 1;
                x = g.put(y, x, &format!("{:<10}", e.name), Role::Title) + 1;
                x = g.put(y, x, &format!("{service},{st:<4}"), Role::Muted) + 1;
                x = g.put(y, x, &format!("{:<7}", fenix_mib::types::apid(apid, ctx.apid_hex)), Role::Muted) + 1;
                let right = if d.kind == Kind::Telecommand {
                    let ack = fenix_mib::types::parse_int(r.clean("CCF_ACK")).unwrap_or(0) as u8;
                    let v: Vec<String> = fenix_ccsds::pus::verification_reports(ack).iter().map(|(s, _)| format!("1,{s}")).collect();
                    if v.is_empty() { String::new() } else { format!("verified by {}", v.join(" ")) }
                } else {
                    match r.clean("PID_PI1_VAL") {
                        "" | "0" => String::new(),
                        v => format!("PI1 {v}"),
                    }
                };
                let desc = if sub.is_empty() { e.description.clone() } else { format!("{sub} -- {}", e.description) };
                g.put(y, x, &fit(&desc, (left + width).saturating_sub(x + right.chars().count() + 2)), Role::Text);
                g.put(y, (left + width).saturating_sub(right.chars().count()), &right, Role::Muted);
                if *i == sel {
                    g.focus(y, left..left + width);
                    anchor = y;
                }
            }
        }
        y += 1;
    }
    let below = rows[end..].iter().filter(|r| matches!(r, SvcRow::Def(..))).count();
    if below > 0 {
        g.put(y, left, &format!("↓ {} more -- ] next service, z the index", thousands(below)), Role::Muted);
    }
    anchor
}

/// `f`'s form, floating under the search line.
fn form_popup(g: &mut Grid, page: &MibPage, left: usize) {
    let Some(form) = &page.form else { return };
    let what = if page.services { "the services" } else { page.tab.plural() };
    let mut rows = vec![vec![(format!("Filter {}", what.to_lowercase()), Role::Title)], Vec::new()];
    for (i, field) in form.fields.iter().enumerate() {
        let cur = i == form.at;
        let label = (format!("{:<14}", field.label), if cur { Role::Title } else { Role::Muted });
        let value = if cur {
            (with_caret(&field.value, form.caret, 30), Role::Title)
        } else if field.value.is_empty() {
            ("any".to_string(), Role::Muted)
        } else {
            (fit(&field.value, 30), Role::Accent)
        };
        rows.push(vec![label, value]);
    }
    rows.push(Vec::new());
    let query = form.query();
    rows.push(vec![(if query.is_empty() { "/ (nothing yet)".to_string() } else { format!("/ {}", fit(&query, 44)) }, Role::Muted)]);
    rows.push(vec![("numbers in hex or decimal: 0x3F2 is 1010".into(), Role::Muted)]);
    rows.push(vec![("Tab next · Enter keep · Esc cancel".into(), Role::Muted)]);
    g.popup = Some(Popup { line: 2, col: left + 2, rows });
}

fn empty(g: &mut Grid, ctx: &Ctx, left: usize, width: usize) {
    let lines: Vec<(String, Role)> = if ctx.loading {
        vec![("Reading the MIBs…".into(), Role::Muted)]
    } else {
        vec![
            ("No MIB for this project.".into(), Role::Title),
            (String::new(), Role::Text),
            ("A project lists its MIB folders in its settings (mib.roots in .fenix/settings.toml), so everyone working on it uses the same ones. A project that is a MIB -- .dat tables at its root or in mib/ -- needs nothing.".into(), Role::Text),
            (String::new(), Role::Text),
            ("a  add a MIB folder to this project's settings".into(), Role::Accent),
            ("A  add one to your own settings, for every project without its own".into(), Role::Accent),
        ]
    };
    let mut y = 4;
    for (text, role) in lines {
        for line in wrap(&text, width.min(90)) {
            g.put(y, left, &line, role);
            y += 1;
        }
        if text.is_empty() {
            y += 1;
        }
    }
}

fn problems(g: &mut Grid, set: &MibSet, shown: &[&fenix_mib::Problem], at: usize, left: usize, width: usize, top: usize) {
    let mut y = top;
    let all = set.problems().len();
    let count = if shown.len() == all { all.to_string() } else { format!("{} of {all}", shown.len()) };
    g.heading(y, left, width, &format!("Problems · {count}"));
    y += 1;
    if shown.is_empty() {
        g.put(y, left + 1, "Nothing matches -- Esc clears the search.", Role::Muted);
    }
    for (i, p) in shown.iter().enumerate() {
        let file = p.file.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        let at_line = p.line.map(|l| format!("{file}:{l}")).unwrap_or(file);
        let label = set.root_label(p.root);
        let mut x = g.put(y, left + 1, &format!("{label:<10}"), Role::Muted) + 1;
        x = g.put(y, x, &format!("{at_line:<16}"), Role::Accent) + 1;
        g.put(y, x, &fit(&p.message, (left + width).saturating_sub(x)), Role::Text);
        if i == at {
            g.focus(y, left..left + width);
        }
        y += 1;
    }
}

fn help(g: &mut Grid, line: usize, left: usize) {
    let groups: [(&str, &[(&str, &str)]); 3] = [
        ("Move", &[("1-5 Tab", "tabs"), ("j k g G", "rows"), ("d u", "20 rows down, up"), ("/", "search: words, field:value"), ("f", "filter with a form"), ("o", "sort by the next column"), ("m", "one MIB, all MIBs"), ("q", "close")]),
        ("A row", &[("Enter", "open its page"), ("i", "insert the telecommand"), ("y", "copy its name"), ("p", "preview on, off")]),
        ("The MIBs", &[("6", "by PUS service"), ("] [ z", "next, previous service, index"), ("!", "problems in their files and against the standards"), ("R", "read them again"), ("a A", "this project's MIB settings, yours")]),
    ];
    let mut rows = vec![vec![("Keys".to_string(), Role::Title)]];
    for (title, keys) in groups {
        rows.push(Vec::new());
        rows.push(vec![(title.to_uppercase(), Role::Muted)]);
        for (k, what) in keys {
            rows.push(vec![(format!("{k:<10}"), Role::Accent), (what.to_string(), Role::Text)]);
        }
    }
    rows.push(Vec::new());
    rows.push(vec![("Filters: type stype apid subsys unit ptc pfc spid critical mib cal, or a column (ccf_critical:Y)".into(), Role::Muted)]);
    rows.push(vec![("any key closes this".into(), Role::Muted)]);
    g.popup = Some(Popup { line, col: left + 4, rows });
}

/// The colour a kind's tag is drawn in.
pub fn kind_role() -> Role {
    Role::Kind(ProjectKind::Mib)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use fenix_mib::MibRoot;

    pub(crate) fn fixture() -> MibRoot {
        let dir = std::env::temp_dir().join(format!("fenix-mib-page-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |t: &str, c: &str| std::fs::write(dir.join(format!("{t}.dat")), c).unwrap();
        w("ccf", "ZTC08101\tSet heater control mode\t\t\tN\t\t8\t1\t1010\t2\t\t\t\t\tTCS\nZTC17001\tConnection test\t\t\tN\t\t17\t1\t1008\t0\n");
        w("cdf", "ZTC08101\tE\tLine\t8\t0\t0\tPTH00101\t\t\t\nZTC08101\tE\tMode\t8\t8\t0\tPTH00102\t\t\t\n");
        w("cpc", "PTH00101\tHeater line\t3\t4\t\t\t\t\tPRF00017\nPTH00102\tHeater control mode\t2\t8\t\t\t\t\t\t\tPAF00042\t\tAUTO\n");
        w("paf", "PAF00042\tHeater mode\tU\t3\n");
        w("pas", "PAF00042\tOFF\t0\nPAF00042\tON\t1\nPAF00042\tAUTO\t2\n");
        w("prf", "PRF00017\tHeater lines\t\t\t\t1\t\n");
        w("prv", "PRF00017\t1\t8\n");
        w("pcf", "NTH00123\tHeater 3 temperature\t\tdegC\t3\t12\n");
        MibRoot { label: "OPS".into(), path: dir }
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn ctx(set: &MibSet) -> Ctx<'_> {
        Ctx { set: Some(set), loading: false, apid_hex: true, source: "your settings".into() }
    }

    #[test]
    fn the_list_shows_each_kinds_columns_and_the_preview_the_selected_row() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let text = layout(&page, &ctx(&set), 160).text;
        assert!(text.contains("1 Telecommands 2"), "{text}");
        assert!(text.contains("ZTC08101  8,1   0x3F2  2") && text.contains("0x3F0"), "{text}");
        assert!(text.contains("Set heater control mode"));
        assert!(text.contains("PTH00102"), "the preview lists its parameters: {text}");
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn searching_narrows_every_tab_and_enter_opens_the_row() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('/'), &c);
        for ch in "heater".chars() {
            page.key(Key::Char(ch), &c);
        }
        assert!(page.typing());
        page.key(Key::Enter, &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.contains("1 Telecommands 1") && text.contains("2 TC parameters 2") && text.contains("4 TM parameters 1"), "{text}");
        page.key(Key::Char('2'), &c);
        page.key(Key::Char('j'), &c);
        let Action::Open(def) = page.key(Key::Enter, &c) else { panic!("Enter opens") };
        assert_eq!(set.get(def).name, "PTH00102");
        assert!(matches!(page.key(Key::Char('i'), &c), Action::None), "no insert from a parameter");
        page.key(Key::Char('1'), &c);
        assert!(matches!(page.key(Key::Char('i'), &c), Action::Insert(_)));
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_filter_completes_and_sorting_cycles_the_columns() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('/'), &c);
        for ch in "ap".chars() {
            page.key(Key::Char(ch), &c);
        }
        page.key(Key::Tab, &c);
        for ch in "1008".chars() {
            page.key(Key::Char(ch), &c);
        }
        page.key(Key::Enter, &c);
        assert_eq!(page.hits(&set, Kind::Telecommand).len(), 1);
        page.key(Key::Escape, &c);
        page.key(Key::Char('/'), &c);
        page.key(Key::Escape, &c);
        assert_eq!(page.hits(&set, Kind::Telecommand).len(), 2);
        for _ in 0..3 {
            page.key(Key::Char('o'), &c);
        }
        assert!(layout(&page, &c, 160).text.contains("APID ↓"));
        let names: Vec<String> = page.hits(&set, Kind::Telecommand).iter().map(|h| set.get(h.def).name.clone()).collect();
        assert_eq!(names, vec!["ZTC17001", "ZTC08101"], "by APID, as numbers");
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn the_services_tab_groups_by_pus_service() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('6'), &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.contains("8 · FUNCTION MANAGEMENT · 1 TC · 0 TM"), "{text}");
        assert!(text.contains("17 · TEST · 1 TC · 0 TM"), "{text}");
        assert!(text.contains("perform a function -- Set heater control mode"), "{text}");
        page.key(Key::Char('j'), &c);
        let Action::Open(def) = page.key(Key::Enter, &c) else { panic!() };
        assert_eq!(set.get(def).name, "ZTC17001");
        page.key(Key::Char('1'), &c);
        assert!(!page.services);
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn the_filter_form_writes_the_search_and_esc_puts_it_back() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('f'), &c);
        assert!(page.typing(), "the form has the keyboard");
        let text = layout(&page, &c, 160).all_text();
        assert!(text.contains("Filter telecommands") && text.contains("APID"), "{text}");
        // It opens on the first filter, Service; APID is two down.
        page.key(Key::Tab, &c);
        page.key(Key::Tab, &c);
        for ch in "0x3F0".chars() {
            page.key(Key::Char(ch), &c);
        }
        assert_eq!(page.hits(&set, Kind::Telecommand).len(), 1, "narrowed as it's typed");
        page.key(Key::Enter, &c);
        assert!(!page.typing());
        assert_eq!(page.query, "apid:0x3F0");
        assert_eq!(set.get(page.hits(&set, Kind::Telecommand)[0].def).name, "ZTC17001");

        // Reopened, it reads the search back; Esc leaves it as it was.
        page.key(Key::Char('f'), &c);
        assert_eq!(page.form.as_ref().unwrap().fields[3].value, "0x3F0");
        page.key(Key::Tab, &c);
        page.key(Key::CtrlC, &c);
        assert_eq!(page.hits(&set, Kind::Telecommand).len(), 2);
        page.key(Key::Escape, &c);
        assert_eq!(page.query, "apid:0x3F0");
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn services_are_jumped_between_and_folded_into_an_index() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('6'), &c);
        page.key(Key::Char(']'), &c);
        let Action::Open(def) = page.key(Key::Enter, &c) else { panic!("Enter opens") };
        assert_eq!(set.get(def).name, "ZTC17001", "] goes to the next service");
        page.key(Key::Char('['), &c);
        let Action::Open(def) = page.key(Key::Enter, &c) else { panic!() };
        assert_eq!(set.get(def).name, "ZTC08101");

        page.key(Key::Char('z'), &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.contains("  8  function management") && text.contains(" 17  test"), "{text}");
        assert!(text.contains("APID 0x3F2") && !text.contains("Set heater control mode"), "one line a service: {text}");
        page.key(Key::Char('j'), &c);
        page.key(Key::Enter, &c);
        let Action::Open(def) = page.key(Key::Enter, &c) else { panic!("unfolded, on the service picked") };
        assert_eq!(set.get(def).name, "ZTC17001");

        // The form on the services: one service by its number.
        page.key(Key::Char('f'), &c);
        for ch in "8".chars() {
            page.key(Key::Char(ch), &c);
        }
        page.key(Key::Enter, &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.contains("8 · FUNCTION MANAGEMENT") && !text.contains("17 · TEST"), "{text}");
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_big_services_list_lays_out_only_the_rows_around_the_selection() {
        let dir = std::env::temp_dir().join(format!("fenix-mib-page-big-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&dir).unwrap();
        let ccf: String = (0..2000).map(|i| format!("ZTC{i:05}\tCommand {i}\t\t\tN\t\t{}\t1\t{}\t0\n", 1 + i % 20, 1000 + i % 7)).collect();
        std::fs::write(dir.join("ccf.dat"), ccf).unwrap();
        let root = MibRoot { label: "BIG".into(), path: dir.clone() };
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('6'), &c);
        page.key(Key::Char('G'), &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.lines().count() < WINDOW + 40, "{} lines", text.lines().count());
        assert!(text.contains("↑ ") && text.contains("20 · "), "the last service, with what's above counted: {}", &text[..400]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_search_narrows_the_services_and_the_problems_too() {
        let root = fixture();
        std::fs::write(root.path.join("paf.dat"), format!("PAF00042\tHeater mode\tU\t3{}\n", "\tx".repeat(30))).unwrap();
        let set = MibSet::load(vec![root.clone()], None);
        let mut page = MibPage::new(MibKey { roots: vec![root.clone()], ..Default::default() });
        let c = ctx(&set);
        page.key(Key::Char('6'), &c);
        page.key(Key::Char('/'), &c);
        for ch in "heater".chars() {
            page.key(Key::Char(ch), &c);
        }
        page.key(Key::Enter, &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.contains("8 · FUNCTION MANAGEMENT") && !text.contains("17 · TEST"), "{text}");
        // A service's own name finds all of it.
        page.key(Key::Escape, &c);
        page.key(Key::Char('/'), &c);
        for ch in "test".chars() {
            page.key(Key::Char(ch), &c);
        }
        page.key(Key::Enter, &c);
        let text = layout(&page, &c, 160).text;
        assert!(text.contains("17 · TEST") && !text.contains("FUNCTION MANAGEMENT"), "{text}");
        assert!(!set.problems().is_empty(), "the fixture has a problem to find");
        {
            page.key(Key::Char('1'), &c);
            page.key(Key::Char('!'), &c);
            page.key(Key::Char('/'), &c);
            for ch in "no-such-problem".chars() {
                page.key(Key::Char(ch), &c);
            }
            let text = layout(&page, &c, 160).text;
            assert!(text.contains(&format!("PROBLEMS · 0 OF {}", set.problems().len())), "{text}");
        }
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn with_no_mib_it_says_how_to_add_one() {
        let page = MibPage::new(MibKey::default());
        let c = Ctx { set: None, loading: false, apid_hex: true, source: String::new() };
        assert!(layout(&page, &c, 120).text.contains("No MIB for this project."));
        let mut page = page;
        assert_eq!(page.key(Key::Char('a'), &c), Action::Settings(true));
    }
}
