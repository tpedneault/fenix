//! A MIB definition as a page: `Enter` on the MIB page, `gd` on a name
//! in a script, a pick from `SPC k t`/`p`/`m`/`n`/`c`. The same shape
//! for every kind -- a header, facts with their codes decoded, then
//! sections (parameters, calibration, limits, what uses it) -- and every
//! name on it is a link: `Enter` follows it in the same page, `Ctrl-o`/
//! `Ctrl-i` (or `H`/`L`) go back and forward, like a browser.
//!
//! The page is built as a `Doc` first, so the keys and the layout agree
//! on where the links are.

use std::path::PathBuf;

use fenix_mib::detail::{self, CalBody};
use fenix_mib::{CalKind, DefRef, Kind, MibSet};

use crate::mib_page::{kind_role, MibKey};
use crate::page::{fit, frame, wrap, Grid, Key, Page, Popup, Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Def(DefRef),
    /// A row in a `.dat` file.
    Row(PathBuf, usize),
}

/// Text at a column of a line, maybe a link.
#[derive(Debug, Clone)]
struct Cell {
    at: usize,
    text: String,
    role: Role,
    target: Option<Target>,
}

#[derive(Debug, Clone)]
enum Line {
    /// A section heading, and a count or note at its right.
    Heading(String, String),
    Cells(Vec<Cell>),
    /// Wrapped to the page's width.
    Para(String, Role),
    /// A bar of segments, sized by `weight`; `focus` names the element
    /// each belongs to.
    Bar(Vec<(String, usize, Role, Option<usize>)>),
    Blank,
}

#[derive(Default)]
struct Doc {
    lines: Vec<Line>,
    /// For a telecommand: which parameter row is element `i`.
    element_rows: Vec<usize>,
}

impl Doc {
    fn heading(&mut self, title: &str, right: impl Into<String>) {
        if !self.lines.is_empty() {
            self.lines.push(Line::Blank);
        }
        self.lines.push(Line::Heading(title.to_string(), right.into()));
    }

    fn row(&mut self, cells: Vec<Cell>) {
        self.lines.push(Line::Cells(cells));
    }

    fn para(&mut self, text: impl Into<String>, role: Role) {
        self.lines.push(Line::Para(text.into(), role));
    }

    /// The links, in reading order: (line, cell).
    fn links(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            if let Line::Cells(cells) = line {
                for (j, c) in cells.iter().enumerate() {
                    if c.target.is_some() {
                        out.push((i, j));
                    }
                }
            }
        }
        out
    }

    fn target(&self, (line, cell): (usize, usize)) -> Option<&Target> {
        match &self.lines[line] {
            Line::Cells(cells) => cells[cell].target.as_ref(),
            _ => None,
        }
    }
}

fn c(at: usize, text: impl Into<String>, role: Role) -> Cell {
    Cell { at, text: text.into(), role, target: None }
}

fn link(at: usize, text: impl Into<String>, def: DefRef) -> Cell {
    Cell { at, text: text.into(), role: Role::Accent, target: Some(Target::Def(def)) }
}

/// A name that should link but points at nothing in this MIB.
fn missing(at: usize, text: &str) -> Cell {
    c(at, format!("{text} (not in this MIB)"), Role::Bad)
}

fn yes_no(v: &str) -> String {
    match v {
        "Y" | "y" => "yes".into(),
        "N" | "n" | "" => "no".into(),
        other => other.to_string(),
    }
}

/// A type with its code beside it: `uint16 · PTC 3/12`.
fn type_text(ptc: &str, pfc: &str) -> String {
    let t = fenix_mib::types::decode(ptc, pfc);
    if t.name.is_empty() {
        String::new()
    } else if t.name.starts_with("PTC") {
        t.name
    } else {
        format!("{} · PTC {ptc}/{pfc}", t.name)
    }
}

/// `(label, value)` facts for a definition, in the order they're shown.
fn facts(set: &MibSet, def: DefRef, apid_hex: bool) -> Vec<(&'static str, String)> {
    let e = set.get(def);
    let r = &e.row;
    let apid = |raw: &str| {
        let (h, d) = (fenix_mib::types::apid(raw, true), fenix_mib::types::apid(raw, false));
        if h == d { h } else if apid_hex { format!("{h} · {d}") } else { format!("{d} · {h}") }
    };
    let v = match def.kind {
        Kind::Telecommand => vec![
            ("PUS", format!("{},{}", r.clean("CCF_TYPE"), r.clean("CCF_STYPE"))),
            ("APID", apid(r.clean("CCF_APID"))),
            ("subsystem", r.clean("CCF_SUBSYS").to_string()),
            ("critical", yes_no(r.clean("CCF_CRITICAL"))),
            ("type", r.clean("CCF_CTYPE").to_string()),
            ("ack flags", r.clean("CCF_ACK").to_string()),
            ("interlock", [r.clean("CCF_ILSCOPE"), r.clean("CCF_ILSTAGE")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" / ")),
            ("packet", r.clean("CCF_PKTID").to_string()),
            ("high priority", r.clean("CCF_HIPRI").to_string()),
            ("map id", r.clean("CCF_MAPID").to_string()),
        ],
        Kind::TcParam => vec![
            ("type", type_text(r.clean("CPC_PTC"), r.clean("CPC_PFC"))),
            ("unit", r.clean("CPC_UNIT").to_string()),
            ("default", r.clean("CPC_DEFVAL").to_string()),
            ("radix", r.clean("CPC_RADIX").to_string()),
            ("display", r.clean("CPC_DISPFMT").to_string()),
            ("category", r.clean("CPC_CATEG").to_string()),
            ("interpretation", r.clean("CPC_INTER").to_string()),
        ],
        Kind::TmPacket => vec![
            ("name", e.alias.clone()),
            ("PUS", format!("{},{}", r.clean("PID_TYPE"), r.clean("PID_STYPE"))),
            ("APID", apid(r.clean("PID_APID"))),
            ("PI1 / PI2", format!("{} / {}", r.clean("PID_PI1_VAL"), r.clean("PID_PI2_VAL"))),
            ("size", set.index().first_row_by_field("tpcf", "TPCF_SPID", &e.name, Some(e.root)).map(|t| t.clean("TPCF_SIZE").to_string()).unwrap_or_default()),
            ("header", r.clean("PID_DFHSIZE").to_string()),
            ("time", r.clean("PID_TIME").to_string()),
            ("event", [r.clean("PID_EVENT"), r.clean("PID_EVID")].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" ")),
        ],
        Kind::TmParam => vec![
            ("type", type_text(r.clean("PCF_PTC"), r.clean("PCF_PFC"))),
            ("unit", r.clean("PCF_UNIT").to_string()),
            ("category", match r.clean("PCF_CATEG") { "N" => "numeric".into(), "S" => "status".into(), "T" => "text".into(), o => o.to_string() }),
            ("nature", match r.clean("PCF_NATUR") { "R" => "raw".into(), "D" => "dynamic".into(), "P" => "synthetic".into(), "H" => "hard-coded".into(), "S" => "saved".into(), "C" => "constant".into(), o => o.to_string() }),
            ("subsystem", r.clean("PCF_SUBSYS").to_string()),
            ("width", r.clean("PCF_WIDTH").to_string()),
            ("PID", r.clean("PCF_PID").to_string()),
            ("related", r.clean("PCF_RELATED").to_string()),
        ],
        Kind::Calibration => {
            let p = r.table.to_uppercase();
            vec![
                ("kind", e.cal.map(|k| k.label()).unwrap_or("").to_string()),
                ("unit", e.cols.get(2).cloned().unwrap_or_default()),
                ("raw format", r.clean(&format!("{p}_RAWFMT")).to_string()),
                ("eng. format", r.clean(&format!("{p}_ENGFMT")).to_string()),
                ("radix", r.clean(&format!("{p}_RADIX")).to_string()),
                ("interpolation", r.clean(&format!("{p}_INTER")).to_string()),
            ]
        }
    };
    v.into_iter().filter(|(_, v)| !v.trim().is_empty() && v.trim() != "/" && v.trim() != ",").collect()
}

/// A calibration's content as lines.
fn cal_lines(doc: &mut Doc, set: &MibSet, cal: DefRef) {
    match detail::calibration(set, cal) {
        CalBody::Points(points) => {
            let engs: Vec<f64> = points.iter().filter_map(|(_, e)| e.parse().ok()).collect();
            if engs.len() > 1 {
                doc.row(vec![c(2, "curve", Role::Muted), c(16, spark(&engs), Role::Good)]);
            }
            doc.row(vec![c(2, "RAW", Role::Muted), c(16, "ENGINEERING", Role::Muted)]);
            for (raw, eng) in points.iter().take(40) {
                doc.row(vec![c(2, raw, Role::Text), c(16, eng, Role::Text)]);
            }
            if points.len() > 40 {
                doc.row(vec![c(2, format!("… {} more points", points.len() - 40), Role::Muted)]);
            }
        }
        CalBody::Texts(texts) => {
            doc.row(vec![c(2, "RAW", Role::Muted), c(16, "TEXT", Role::Muted)]);
            for (raw, text) in texts {
                doc.row(vec![c(2, raw, Role::Text), c(16, text, Role::Title)]);
            }
        }
        CalBody::Ranges(ranges) => {
            for (lo, hi) in ranges {
                doc.row(vec![c(2, format!("{lo} .. {hi}"), Role::Text)]);
            }
        }
        CalBody::Coefficients(k) => {
            let terms: Vec<String> = k.iter().enumerate().map(|(i, a)| match i {
                0 => a.clone(),
                1 => format!("{a}·x"),
                n => format!("{a}·x^{n}"),
            }).collect();
            let what = if set.get(cal).cal == Some(CalKind::Logarithmic) { "1/y = " } else { "y = " };
            doc.row(vec![c(2, format!("{what}{}", terms.join(" + ")), Role::Text)]);
        }
    }
}

/// Eight-level bars for a curve's values.
fn spark(values: &[f64]) -> String {
    let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    values.iter().take(60).map(|v| if hi > lo { bars[(((v - lo) / (hi - lo)) * 7.0).round() as usize] } else { bars[3] }).collect()
}

fn used_by_rows(doc: &mut Doc, set: &MibSet, def: DefRef) {
    let users = set.used_by(def);
    let title = match def.kind {
        Kind::TcParam => "Used by telecommands",
        Kind::TmParam => "Carried by",
        Kind::Calibration => "Used by parameters",
        _ => "Used by",
    };
    if def.kind == Kind::TmParam {
        let carried = detail::carried_by(set, def);
        doc.heading(title, format!("{} packets", carried.len()));
        if carried.is_empty() {
            doc.para("No packet carries it.", Role::Muted);
        }
        for (packet, slot) in carried {
            let p = set.get(packet);
            let at = format!("byte {} bit {}{}", slot.byte.map(|b| b.to_string()).unwrap_or("?".into()), slot.bit.unwrap_or(0), slot.bits.map(|b| format!(" · {b} bits")).unwrap_or_default());
            doc.row(vec![link(2, &p.name, packet), c(12, &p.alias, Role::Muted), c(34, at, Role::Text)]);
        }
        return;
    }
    doc.heading(title, users.len().to_string());
    if users.is_empty() {
        doc.para("Nothing in the MIB refers to it.", Role::Muted);
    }
    for &u in users {
        let e = set.get(u);
        let mut cells = vec![c(2, u.kind.tag(), kind_role()), link(7, &e.name, u)];
        if def.kind == Kind::TcParam {
            let me = &set.get(def).name;
            if let Some(cdf) = set.index().rows_by_field("cdf", "CDF_CNAME", &e.name, Some(e.root)).into_iter().find(|r| r.clean("CDF_PNAME") == me) {
                cells.push(c(20, format!("bit {}", cdf.clean("CDF_BIT")), Role::Muted));
            }
        }
        cells.push(c(32, &e.description, Role::Text));
        doc.row(cells);
    }
}

/// The whole page for `def`.
fn build(set: &MibSet, def: DefRef, raw: bool) -> Doc {
    let mut doc = Doc::default();
    let e = set.get(def);
    match def.kind {
        Kind::Telecommand => {
            let els = detail::tc_elements(set, def);
            let total: i64 = els.iter().filter_map(|el| Some(el.bit? + el.bits?)).max().unwrap_or(0);
            let fixed = els.iter().filter(|el| el.fixed.is_some() || el.name.is_empty()).count();
            doc.heading("Application data", format!("{total} bits"));
            if total > 0 {
                let mut segs = Vec::new();
                let mut pos = 0;
                for (i, el) in els.iter().enumerate() {
                    let (Some(bit), Some(bits)) = (el.bit, el.bits) else { continue };
                    if bit > pos {
                        segs.push(("".to_string(), (bit - pos) as usize, Role::Muted, None));
                    }
                    let label = if el.name.is_empty() { "spare".to_string() } else { el.description.clone() };
                    let role = if el.fixed.is_some() || el.name.is_empty() { Role::Muted } else if segs.len() % 2 == 0 { Role::Good } else { Role::Accent };
                    segs.push((label, bits.max(1) as usize, role, Some(i)));
                    pos = pos.max(bit + bits);
                }
                doc.lines.push(Line::Bar(segs));
            }
            doc.heading("Parameters", format!("{fixed} fixed · {} to fill", els.len() - fixed));
            doc.row(vec![c(2, "NAME", Role::Muted), c(14, "BITS", Role::Muted), c(23, "TYPE", Role::Muted), c(38, "VALUE", Role::Muted), c(66, "DESCRIPTION", Role::Muted)]);
            for el in &els {
                doc.element_rows.push(doc.lines.len());
                let name = match (el.param, el.name.is_empty()) {
                    (_, true) => c(2, "spare", Role::Muted),
                    (Some(p), false) => link(2, &el.name, p),
                    (None, false) => missing(2, &el.name),
                };
                let bits = match (el.bit, el.bits) {
                    (Some(b), Some(n)) => format!("{b}+{n}"),
                    _ => String::new(),
                };
                let mut cells = vec![name, c(14, bits, Role::Muted), c(23, &el.type_name, Role::Text)];
                match &el.fixed {
                    Some(v) => cells.push(c(38, format!("fixed {v}"), Role::Muted)),
                    None if el.name.is_empty() => {}
                    None => {
                        let mut value = if el.allowed.is_empty() { "to fill".to_string() } else { el.allowed.clone() };
                        if !el.default.is_empty() {
                            value = format!("{value} ({})", el.default);
                        }
                        cells.push(c(38, fit(&value, 18), Role::Good));
                        if let Some(cal) = el.cal {
                            cells.push(link(58, &set.get(cal).name, cal));
                        }
                    }
                }
                let mut desc = el.description.clone();
                if el.group > 0 {
                    desc = format!("{desc} -- the next {} repeat this many times", el.group);
                }
                cells.push(c(66, desc, Role::Text));
                doc.row(cells);
            }
            let seqs = set.sequences_using(def);
            doc.heading("Sent by sequences", seqs.len().to_string());
            if seqs.is_empty() {
                doc.para("No sequence sends it.", Role::Muted);
            }
            for (name, descr, entry) in seqs {
                doc.row(vec![c(2, name, Role::Title), c(14, format!("entry {entry}"), Role::Muted), c(28, descr, Role::Text)]);
            }
        }
        Kind::TcParam => {
            let cals = detail::tc_param_calibrations(set, def);
            doc.heading("Calibration", if cals.is_empty() { "none".to_string() } else { String::new() });
            for field in ["CPC_CCAREF", "CPC_PAFREF", "CPC_PRFREF"] {
                let id = e.row.clean(field);
                if !id.is_empty() && !cals.iter().any(|&c| set.get(c).name == id) {
                    doc.row(vec![missing(2, id)]);
                }
            }
            for cal in cals {
                let ce = set.get(cal);
                doc.row(vec![link(2, &ce.name, cal), c(16, ce.cal.map(|k| k.label()).unwrap_or(""), Role::Muted), c(30, &ce.description, Role::Text)]);
                cal_lines(&mut doc, set, cal);
            }
            used_by_rows(&mut doc, set, def);
        }
        Kind::TmPacket => {
            let slots = detail::packet_slots(set, def);
            doc.heading("Parameters", slots.len().to_string());
            doc.row(vec![c(2, "BYTE.BIT", Role::Muted), c(13, "NAME", Role::Muted), c(25, "BITS", Role::Muted), c(31, "OCC", Role::Muted), c(37, "DESCRIPTION", Role::Muted)]);
            for s in slots {
                let name = match s.param {
                    Some(p) => link(13, &s.name, p),
                    None => missing(13, &s.name),
                };
                let at = format!("{}.{}", s.byte.map(|b| b.to_string()).unwrap_or("?".into()), s.bit.unwrap_or(0));
                doc.row(vec![c(2, at, Role::Muted), name, c(25, s.bits.map(|b| b.to_string()).unwrap_or_default(), Role::Text), c(31, s.occurrences, Role::Muted), c(37, s.description, Role::Text)]);
            }
        }
        Kind::TmParam => {
            let valid = e.row.clean("PCF_VALID");
            if !valid.is_empty() {
                doc.heading("Valid when", "");
                let cell = match set.find(Kind::TmParam, e.root, valid) {
                    Some(p) => link(2, valid, p),
                    None => missing(2, valid),
                };
                let value = e.row.clean("PCF_VALPAR");
                let mut cells = vec![cell];
                if !value.is_empty() {
                    cells.push(c(14, format!("= {value}"), Role::Text));
                }
                doc.row(cells);
            }
            let curtx = e.row.clean("PCF_CURTX");
            match detail::tm_calibration(set, def) {
                Some(cal) => {
                    let ce = set.get(cal);
                    doc.heading("Calibration", ce.cols[1].clone());
                    doc.row(vec![link(2, &ce.name, cal), c(16, ce.cal.map(|k| k.label()).unwrap_or(""), Role::Muted), c(30, &ce.description, Role::Text)]);
                    cal_lines(&mut doc, set, cal);
                }
                None if !curtx.is_empty() => {
                    doc.heading("Calibration", "");
                    doc.row(vec![missing(2, curtx)]);
                }
                None => {}
            }
            let limits = detail::limits(set, def);
            doc.heading("Limits", limits.len().to_string());
            if limits.is_empty() {
                doc.para("Not monitored.", Role::Muted);
            }
            for l in limits {
                let role = match l.kind.as_str() {
                    "soft" => Role::Warn,
                    "hard" => Role::Bad,
                    _ => Role::Text,
                };
                let mut cells = vec![c(2, &l.kind, role), c(10, format!("{} .. {}", l.low, l.high), Role::Text)];
                if let Some((p, v)) = l.when {
                    cells.push(c(30, "when", Role::Muted));
                    cells.push(match set.find(Kind::TmParam, e.root, &p) {
                        Some(d) => link(35, &p, d),
                        None => c(35, &p, Role::Text),
                    });
                    cells.push(c(47, format!("= {v}"), Role::Text));
                }
                doc.row(cells);
            }
            used_by_rows(&mut doc, set, def);
        }
        Kind::Calibration => {
            doc.heading(e.cal.map(|k| k.label()).unwrap_or("Values"), e.cols[1].clone());
            cal_lines(&mut doc, set, def);
            used_by_rows(&mut doc, set, def);
        }
    }
    doc.heading("Raw fields", format!("{} · {}.dat line {} · r {}", e.row.fields.len(), e.row.table, e.row.source.line, if raw { "hides" } else { "shows" }));
    if raw {
        for (name, value) in &e.row.fields {
            doc.row(vec![c(2, name, Role::Muted), c(20, value.trim(), Role::Text)]);
        }
    }
    doc.row(vec![Cell { at: 2, text: format!("{}:{}", e.row.source.file.display(), e.row.source.line), role: Role::Accent, target: Some(Target::Row(e.row.source.file.clone(), e.row.source.line)) }]);
    doc
}

/// The top of a definition's page, for the MIB page's preview.
pub fn preview(set: &MibSet, def: DefRef, apid_hex: bool, width: usize) -> Vec<(String, Role)> {
    let e = set.get(def);
    let mut out = vec![(format!("{}  {}", def.kind.tag(), e.name), Role::Title)];
    for line in wrap(&e.description, width).into_iter().take(3) {
        out.push((line, Role::Text));
    }
    out.push((String::new(), Role::Text));
    for (label, value) in facts(set, def, apid_hex).into_iter().take(8) {
        out.push((format!("{label:<12}{value}"), Role::Muted));
    }
    let doc = build(set, def, false);
    for line in doc.lines.iter().take(24) {
        match line {
            Line::Heading(t, r) => {
                out.push((String::new(), Role::Text));
                out.push((format!("{} · {r}", t.to_uppercase()), Role::Muted));
            }
            Line::Cells(cells) if !cells.iter().any(|c| c.text == "NAME" || c.text == "RAW") => {
                let mut text = String::new();
                for cell in cells {
                    let at = cell.at.saturating_sub(2) / 2;
                    if text.chars().count() < at {
                        text.push_str(&" ".repeat(at - text.chars().count()));
                    } else if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(&cell.text);
                }
                out.push((text, Role::Text));
            }
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Insert(DefRef),
    Copy(String),
    OpenFile(PathBuf, usize),
    /// The MIB page.
    MibPage,
}

pub struct DefPage {
    /// The MIBs it's from: the set it's looked up by.
    pub key: MibKey,
    history: Vec<DefRef>,
    at: usize,
    /// The focused link, in reading order.
    focus: usize,
    raw: bool,
    pending_g: bool,
    help: bool,
    pub note: Option<(String, bool)>,
}

impl DefPage {
    pub fn new(key: MibKey, def: DefRef) -> Self {
        DefPage { key, history: vec![def], at: 0, focus: 0, raw: false, pending_g: false, help: false, note: None }
    }

    pub fn current(&self) -> DefRef {
        self.history[self.at]
    }

    /// Shows `def`, keeping where it came from for `Ctrl-o`.
    pub fn go(&mut self, def: DefRef) {
        if def == self.current() {
            return;
        }
        self.history.truncate(self.at + 1);
        self.history.push(def);
        self.at += 1;
        self.focus = 0;
    }

    pub fn back(&mut self) -> bool {
        if self.at == 0 {
            return false;
        }
        self.at -= 1;
        self.focus = 0;
        true
    }

    pub fn forward(&mut self) -> bool {
        if self.at + 1 >= self.history.len() {
            return false;
        }
        self.at += 1;
        self.focus = 0;
        true
    }

    pub fn key(&mut self, key: Key, set: &MibSet) -> Action {
        self.note = None;
        if self.help {
            self.help = false;
            return Action::None;
        }
        let doc = build(set, self.current(), self.raw);
        let links = doc.links();
        let focus = self.focus.min(links.len().saturating_sub(1));
        if std::mem::take(&mut self.pending_g) {
            match key {
                Key::Char('g') => self.focus = 0,
                Key::Char('f') => {
                    let r = &set.get(self.current()).row.source;
                    return Action::OpenFile(r.file.clone(), r.line);
                }
                _ => {}
            }
            return Action::None;
        }
        let line_of = |i: usize| links.get(i).map(|l| l.0);
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('?') => self.help = true,
            Key::Char('g') => self.pending_g = true,
            Key::Char('G') => self.focus = links.len().saturating_sub(1),
            // Down a line: the first link on the next line with one.
            Key::Down | Key::Char('j') => {
                if let Some(i) = (focus + 1..links.len()).find(|&i| line_of(i) != line_of(focus)) {
                    self.focus = i;
                }
            }
            Key::Up | Key::Char('k') => {
                let here = line_of(focus);
                if let Some(prev) = (0..focus).rev().find(|&i| line_of(i) != here) {
                    let line = line_of(prev);
                    self.focus = (0..=prev).find(|&i| line_of(i) == line).unwrap_or(prev);
                }
            }
            Key::Right | Key::Char('l') | Key::Tab => self.focus = (focus + 1).min(links.len().saturating_sub(1)),
            Key::Left | Key::Char('h') | Key::BackTab => self.focus = focus.saturating_sub(1),
            Key::Enter => match links.get(focus).and_then(|&l| doc.target(l)) {
                Some(Target::Def(d)) => self.go(*d),
                Some(Target::Row(path, line)) => return Action::OpenFile(path.clone(), *line),
                None => {}
            },
            Key::CtrlO | Key::Backspace | Key::Char('H') => {
                if !self.back() {
                    self.note = Some(("nothing before this one".into(), false));
                }
            }
            Key::CtrlI | Key::Char('L') => {
                if !self.forward() {
                    self.note = Some(("nothing after this one".into(), false));
                }
            }
            Key::Char('u') => {
                let used = doc.lines.iter().position(|l| matches!(l, Line::Heading(t, _) if t.starts_with("Used by") || t.starts_with("Carried by")));
                match used.and_then(|u| links.iter().position(|&(line, _)| line > u)) {
                    Some(i) => self.focus = i,
                    None => self.note = Some(("nothing refers to it".into(), false)),
                }
            }
            Key::Char('r') => self.raw = !self.raw,
            Key::Char('i') => {
                if self.current().kind == Kind::Telecommand {
                    return Action::Insert(self.current());
                }
                self.note = Some(("i inserts a telecommand -- open one first".into(), true));
            }
            Key::Char('y') => return Action::Copy(set.get(self.current()).name.clone()),
            Key::Char('m') | Key::Char('/') => return Action::MibPage,
            _ => {}
        }
        Action::None
    }
}

pub fn title(set: Option<&MibSet>, page: &DefPage) -> String {
    match set {
        Some(set) if page.current().index < set.entries(page.current().kind).len() => format!("*mib: {}*", set.get(page.current()).name),
        _ => "*mib*".to_string(),
    }
}

pub fn layout(page: &DefPage, set: Option<&MibSet>, apid_hex: bool, cols: usize) -> Page {
    let (left, width) = frame(cols, 150);
    let mut g = Grid::new();
    let Some(set) = set.filter(|s| page.current().index < s.entries(page.current().kind).len()) else {
        g.put(1, left, "This MIB isn't loaded any more -- SPC k k opens the MIB page.", Role::Muted);
        return g.finish();
    };
    let def = page.current();
    let e = set.get(def);
    let crumb = format!("MIB › {}", def.kind.plural());
    g.put(1, left, &crumb, Role::Muted);
    let trail = format!("{} of {}", page.at + 1, page.history.len());
    if page.history.len() > 1 {
        g.put(1, (left + width).saturating_sub(trail.chars().count()), &trail, Role::Muted);
    }
    let mut x = g.put(2, left, def.kind.tag(), kind_role()) + 2;
    x = g.put(2, x, &e.name, Role::Title) + 2;
    if !e.alias.is_empty() {
        x = g.put(2, x, &e.alias, Role::Text) + 2;
    }
    g.put(2, x, &fit(&e.description, (left + width).saturating_sub(x + 30)), Role::Text);
    let source = format!("{} · {}.dat:{}", set.root_label(e.root), e.row.table, e.row.source.line);
    g.put(2, (left + width).saturating_sub(source.chars().count()), &source, Role::Muted);
    let mut y = 3;
    for extra in ["CCF_DESCR2", "CPC_DESCR2", "PCF_DESCR2"] {
        let d = e.row.clean(extra);
        if !d.is_empty() {
            for line in wrap(d, width) {
                g.put(y, left, &line, Role::Muted);
                y += 1;
            }
        }
    }
    if let Some((text, bad)) = &page.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    y += 1;
    let facts = facts(set, def, apid_hex);
    let per_row = if width >= 120 { 4 } else { 2 };
    let cw = width / per_row;
    for chunk in facts.chunks(per_row) {
        for (i, (label, value)) in chunk.iter().enumerate() {
            let x = left + i * cw;
            let vx = g.put(y, x, label, Role::Muted) + 1;
            g.put(y, vx, &fit(value, (x + cw).saturating_sub(vx + 2)), Role::Text);
        }
        y += 1;
    }

    let doc = build(set, def, page.raw);
    let links = doc.links();
    let focus = links.get(page.focus.min(links.len().saturating_sub(1))).copied();
    // Which telecommand element the focus is on, for the bar.
    let focused_el = focus.and_then(|(line, _)| doc.element_rows.iter().position(|&r| r == line));
    let mut anchor = y;
    for (i, line) in doc.lines.iter().enumerate() {
        match line {
            Line::Blank => {}
            Line::Heading(title, right) => {
                y += 1;
                let text = if right.is_empty() { title.clone() } else { format!("{title} · {right}") };
                g.heading(y, left, width, &text);
            }
            Line::Para(text, role) => {
                for l in wrap(text, width - 2) {
                    g.put(y, left + 2, &l, *role);
                    y += 1;
                }
                continue;
            }
            Line::Cells(cells) => {
                let mut end = left;
                for (j, cell) in cells.iter().enumerate() {
                    let x = left + cell.at;
                    let room = cells.get(j + 1).map(|n| left + n.at - 1).unwrap_or(left + width).saturating_sub(x);
                    let e = g.put(y, x, &fit(&cell.text, room), cell.role);
                    if focus == Some((i, j)) {
                        g.focus(y, left..left + width);
                        if cells.iter().filter(|c| c.target.is_some()).count() > 1 {
                            g.panels.push((y, x..e));
                        }
                        anchor = y;
                    }
                    end = e;
                }
                let _ = end;
            }
            Line::Bar(segs) => {
                let total: usize = segs.iter().map(|s| s.1).sum::<usize>().max(1);
                let bw = width - 2;
                let mut x = left + 2;
                let mut acc = 0;
                let mut ruler = vec![(x, 0usize)];
                for (label, weight, role, el) in segs {
                    let next = left + 2 + (acc + weight) * bw / total;
                    let w = next.saturating_sub(x).max(1);
                    let text = format!("[{:^w$}]", fit(label, w.saturating_sub(2)), w = w.saturating_sub(2));
                    let on = el.is_some() && *el == focused_el;
                    let e = g.put(y, x, &fit(&text, w), if on { Role::Title } else { *role });
                    if on {
                        g.panels.push((y, x..e));
                    }
                    acc += weight;
                    x = next;
                    ruler.push((x, acc));
                }
                y += 1;
                let mut last = 0;
                for (rx, bit) in ruler {
                    let s = bit.to_string();
                    let at = rx.min(left + width - s.len());
                    if at >= last {
                        last = g.put(y, at, &s, Role::Muted) + 1;
                    }
                }
            }
        }
        y += 1;
    }

    if page.help {
        let rows: Vec<Vec<(String, Role)>> = [
            ("j k h l", "move between links"),
            ("Enter", "follow the link"),
            ("Ctrl-o H", "back"),
            ("Ctrl-i L", "forward"),
            ("u", "to what uses it"),
            ("i", "insert this telecommand"),
            ("y", "copy the name"),
            ("gf", "open its .dat line"),
            ("r", "raw fields"),
            ("m /", "the MIB page"),
            ("q", "close"),
        ]
        .iter()
        .map(|(k, w)| vec![(format!("{k:<10}"), Role::Accent), (w.to_string(), Role::Text)])
        .chain([Vec::new(), vec![("any key closes this".to_string(), Role::Muted)]])
        .collect();
        let mut all = vec![vec![("Keys".to_string(), Role::Title)], Vec::new()];
        all.extend(rows);
        g.popup = Some(Popup { line: anchor, col: left + 4, rows: all });
    }
    let mut keys = vec![("Enter", "follow"), ("Ctrl-o", "back"), ("u", "used by")];
    if def.kind == Kind::Telecommand {
        keys.push(("i", "insert"));
    }
    keys.extend([("y", "copy name"), ("gf", ".dat line"), ("r", "raw"), ("m", "MIB page"), ("?", "all keys"), ("q", "close")]);
    g.keys(left, width, &keys);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> (MibSet, fenix_mib::MibRoot) {
        let root = crate::mib_page::tests::fixture();
        (MibSet::load(vec![root.clone()], None), root)
    }

    #[test]
    fn a_telecommand_page_lays_out_its_bits_parameters_and_links() {
        let (set, root) = set();
        let tc = set.find(Kind::Telecommand, 0, "ZTC08101").unwrap();
        let page = DefPage::new(MibKey { roots: vec![root.clone()], ..Default::default() }, tc);
        let text = layout(&page, Some(&set), true, 160).text;
        assert!(text.contains("TC  ZTC08101  Set heater control mode"), "{text}");
        assert!(text.contains("APID 0x3F2 · 1010"), "{text}");
        assert!(text.contains("APPLICATION DATA · 16 BITS"), "{text}");
        assert!(text.contains("[") && text.contains("Line"), "the bar: {text}");
        assert!(text.contains("PTH00102") && text.contains("OFF ON AUTO (AUTO)"), "{text}");
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn links_are_followed_and_back_and_forward_retrace_them() {
        let (set, root) = set();
        let tc = set.find(Kind::Telecommand, 0, "ZTC08101").unwrap();
        let mut page = DefPage::new(MibKey { roots: vec![root.clone()], ..Default::default() }, tc);
        page.key(Key::Char('j'), &set);
        page.key(Key::Enter, &set);
        assert_eq!(set.get(page.current()).name, "PTH00102");
        let text = layout(&page, Some(&set), true, 160).text;
        assert!(text.contains("PAF00042") && text.contains("AUTO"), "its calibration inline: {text}");
        assert!(text.contains("USED BY TELECOMMANDS · 1"), "{text}");
        page.key(Key::Char('u'), &set);
        page.key(Key::Enter, &set);
        assert_eq!(set.get(page.current()).name, "ZTC08101");
        page.key(Key::CtrlO, &set);
        assert_eq!(set.get(page.current()).name, "PTH00102");
        page.key(Key::CtrlO, &set);
        assert_eq!(page.current(), tc);
        page.key(Key::CtrlI, &set);
        assert_eq!(set.get(page.current()).name, "PTH00102");
        assert_eq!(page.key(Key::Char('y'), &set), Action::Copy("PTH00102".into()));
        page.key(Key::Char('g'), &set);
        assert!(matches!(page.key(Key::Char('f'), &set), Action::OpenFile(_, 2)));
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn the_spark_line_follows_the_curve() {
        assert_eq!(spark(&[0.0, 1.0, 2.0]), "▁▅█");
        assert_eq!(spark(&[3.0, 3.0]), "▄▄");
    }
}
