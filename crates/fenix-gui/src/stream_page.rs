//! `SPC k f` and `SPC k l`: a recording, or a live source, as a page.
//! Every packet one row -- its time, APID, SPID and name, service,
//! sequence count, length, and whether it checks out -- with sequence
//! gaps as lines of their own; `2` counts by APID, `3` follows one
//! parameter through the packets, `4` lists every problem. The host
//! reads (or receives) off the UI thread and hands rows over as they
//! come; this lays them out and answers keys.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::mib_page::{thousands, MibKey};
use crate::page::{fit, fit_tail, frame, Grid, Key, Page, Popup, Role};

/// One packet of the stream.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub offset: usize,
    pub time: Option<String>,
    pub apid: u16,
    pub is_tc: bool,
    pub pus: Option<(u8, u8)>,
    pub seq: u16,
    pub len: usize,
    pub spid: Option<String>,
    /// The packet's name: its TPCF name, the telecommand, a PUS subtype.
    pub label: String,
    /// Why it doesn't check out.
    pub bad: Option<String>,
    pub vc: Option<u8>,
    pub idle: bool,
    /// A verification report: which telecommand it's about.
    pub verifies: Option<String>,
    pub bytes: Vec<u8>,
    /// The CADU or CLTU it came in, as it arrived.
    pub unit: Option<std::sync::Arc<Vec<u8>>>,
}

/// One sample of a parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub row: usize,
    pub time: Option<String>,
    pub raw: String,
    pub value: String,
    pub number: Option<f64>,
    /// `None` within limits, else the limit broken.
    pub off: Option<(String, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Packets,
    Apids,
    Parameter,
    Problems,
}

const TABS: [Tab; 4] = [Tab::Packets, Tab::Apids, Tab::Parameter, Tab::Problems];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Binary,
    Hex,
    Csv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    Inspect(usize),
    PickParameter,
    Write(Write),
    /// Read again with the next framing.
    Reframe,
    /// The hex view at an offset.
    Hex(usize),
    /// The inspector on the CADU or CLTU a row came in.
    Unit(usize),
}

pub struct StreamPage {
    pub key: MibKey,
    pub title: String,
    pub live: bool,
    pub framing: String,
    pub rows: Vec<Row>,
    /// Frames lost: (offset, VC, how many).
    pub lost: Vec<(usize, u8, u64)>,
    /// Bytes skipped: (offset, length, why).
    pub junk: Vec<(usize, usize, String)>,
    pub done: bool,
    /// A live source's state: text, and whether it's healthy.
    pub status: Option<(String, bool)>,
    pub param: Option<(String, Vec<Sample>)>,
    pub stop: Arc<AtomicBool>,
    tab: Tab,
    query: String,
    searching: bool,
    pub show_idle: bool,
    sel: [usize; 4],
    /// Follow the newest row (live).
    follow: bool,
    menu: Option<usize>,
    help: bool,
    pub note: Option<(String, bool)>,
}

/// A line of the packets tab.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Row(usize),
    Gap { apid: u16, from: u16, to: u16, missing: u32 },
}

/// One APID's totals.
#[derive(Debug, Clone, Default)]
struct ApidStats {
    packets: usize,
    gaps: u32,
    missing: u64,
    bad: usize,
    first: Option<String>,
    last: Option<String>,
    spids: Vec<String>,
}

impl StreamPage {
    pub fn new(key: MibKey, title: String, live: bool) -> Self {
        StreamPage {
            key,
            title,
            live,
            framing: String::new(),
            rows: Vec::new(),
            lost: Vec::new(),
            junk: Vec::new(),
            done: false,
            status: None,
            param: None,
            stop: Arc::new(AtomicBool::new(false)),
            tab: Tab::Packets,
            query: String::new(),
            searching: false,
            show_idle: false,
            sel: [0; 4],
            follow: live,
            menu: None,
            help: false,
            note: None,
        }
    }

    pub fn typing(&self) -> bool {
        self.searching
    }

    pub fn paste(&mut self, text: &str) {
        if self.searching {
            self.query.extend(text.chars().filter(|c| !c.is_control()));
        }
    }

    pub fn close(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether `row` passes the search: words in its name, `apid:`,
    /// `type:`, `stype:`, `spid:`, `vc:` and `check:bad`.
    fn matches(&self, row: &Row) -> bool {
        if row.idle && !self.show_idle {
            return false;
        }
        for token in self.query.split_whitespace() {
            let ok = match token.split_once(':') {
                Some((k, v)) => {
                    let num = fenix_mib::types::parse_int(v);
                    match k.to_ascii_lowercase().as_str() {
                        "apid" => num == Some(row.apid as i64),
                        "type" => row.pus.is_some_and(|p| Some(p.0 as i64) == num),
                        "stype" | "subtype" => row.pus.is_some_and(|p| Some(p.1 as i64) == num),
                        "spid" => row.spid.as_deref() == Some(v),
                        "vc" => row.vc.is_some_and(|c| Some(c as i64) == num),
                        "check" | "crc" => (v == "bad") == row.bad.is_some(),
                        "seq" => num == Some(row.seq as i64),
                        _ => false,
                    }
                }
                None => {
                    let t = token.to_lowercase();
                    row.label.to_lowercase().contains(&t) || row.spid.as_deref().is_some_and(|s| s.contains(&t))
                }
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// The packets tab's lines: the rows shown, with sequence gaps.
    fn lines(&self) -> Vec<Line> {
        let mut last: BTreeMap<u16, u16> = BTreeMap::new();
        let mut out = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            let gap = if r.idle || r.is_tc {
                None
            } else {
                let prev = last.insert(r.apid, r.seq);
                prev.and_then(|p| {
                    let missing = (r.seq as u32 + 0x4000 - p as u32 - 1) % 0x4000;
                    (missing > 0).then_some((p, missing))
                })
            };
            if !self.matches(r) {
                continue;
            }
            if let Some((p, missing)) = gap {
                out.push(Line::Gap { apid: r.apid, from: (p + 1) & 0x3FFF, to: r.seq.wrapping_sub(1) & 0x3FFF, missing });
            }
            out.push(Line::Row(i));
        }
        out
    }

    fn stats(&self) -> BTreeMap<u16, ApidStats> {
        let mut out: BTreeMap<u16, ApidStats> = BTreeMap::new();
        let mut last: BTreeMap<u16, u16> = BTreeMap::new();
        for r in self.rows.iter().filter(|r| !r.idle || self.show_idle) {
            let s = out.entry(r.apid).or_default();
            s.packets += 1;
            if r.bad.is_some() {
                s.bad += 1;
            }
            if !r.is_tc {
                if let Some(p) = last.insert(r.apid, r.seq) {
                    let missing = (r.seq as u32 + 0x4000 - p as u32 - 1) % 0x4000;
                    if missing > 0 {
                        s.gaps += 1;
                        s.missing += missing as u64;
                    }
                }
            }
            if s.first.is_none() {
                s.first = r.time.clone();
            }
            if r.time.is_some() {
                s.last = r.time.clone();
            }
            if let Some(spid) = &r.spid {
                if !s.spids.contains(spid) {
                    s.spids.push(spid.clone());
                }
            }
        }
        out
    }

    /// Every problem, as (text, the row it's at).
    fn problems(&self) -> Vec<(String, Option<usize>)> {
        let mut out = Vec::new();
        for l in self.lines() {
            if let Line::Gap { apid, from, to, missing } = l {
                let at = self.rows.iter().position(|r| r.apid == apid && r.seq == (to + 1) & 0x3FFF);
                out.push((format!("APID 0x{apid:03X}: {missing} missing, sequence {from} to {to}"), at));
            }
        }
        for (i, r) in self.rows.iter().enumerate() {
            if let Some(why) = &r.bad {
                out.push((format!("0x{:X} APID 0x{:03X} seq {}: {why}", r.offset, r.apid, r.seq), Some(i)));
            } else if !r.idle && !r.is_tc && r.spid.is_none() && r.pus.is_some() {
                out.push((format!("0x{:X} APID 0x{:03X} {}: not in the MIB", r.offset, r.apid, r.label), Some(i)));
            }
        }
        for (offset, vc, n) in &self.lost {
            out.push((format!("0x{offset:X}: {n} frame{} lost on VC {vc}", if *n == 1 { "" } else { "s" }), None));
        }
        for (offset, len, why) in &self.junk {
            // A frame that arrived with something wrong has no length here.
            out.push((if *len == 0 { format!("0x{offset:X}: {why}") } else { format!("0x{offset:X}: {len} octets skipped -- {why}") }, None));
        }
        if let Some((name, samples)) = &self.param {
            for s in samples.iter().filter(|s| s.off.is_some()) {
                let (why, _) = s.off.clone().unwrap_or_default();
                out.push((format!("{name} = {} at {}: {why}", s.value, s.time.clone().unwrap_or_default()), Some(s.row)));
            }
        }
        out
    }

    fn count(&self) -> usize {
        match self.tab {
            Tab::Packets => self.lines().len(),
            Tab::Apids => self.stats().len(),
            Tab::Parameter => self.param.as_ref().map(|(_, s)| s.len()).unwrap_or(0),
            Tab::Problems => self.problems().len(),
        }
    }

    fn tab_index(&self) -> usize {
        TABS.iter().position(|t| *t == self.tab).unwrap_or(0)
    }

    /// The row under the selection, in whichever tab.
    fn selected_row(&self) -> Option<usize> {
        let s = self.sel[self.tab_index()];
        match self.tab {
            Tab::Packets => match self.lines().get(s)? {
                Line::Row(i) => Some(*i),
                Line::Gap { .. } => None,
            },
            Tab::Parameter => self.param.as_ref()?.1.get(s).map(|x| x.row),
            Tab::Problems => self.problems().get(s)?.1,
            Tab::Apids => {
                let apid = *self.stats().keys().nth(s)?;
                self.rows.iter().position(|r| r.apid == apid)
            }
        }
    }

    pub fn key(&mut self, key: Key) -> Action {
        self.note = None;
        if self.help {
            self.help = false;
            return Action::None;
        }
        if let Some(at) = self.menu {
            let items = self.write_items();
            match key {
                Key::Escape | Key::Char('q') => self.menu = None,
                Key::Down | Key::Char('j') => self.menu = Some((at + 1).min(items.len().saturating_sub(1))),
                Key::Up | Key::Char('k') => self.menu = Some(at.saturating_sub(1)),
                Key::Enter => {
                    self.menu = None;
                    if let Some((_, w)) = items.into_iter().nth(at) {
                        return Action::Write(w);
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        if self.searching {
            match key {
                Key::Escape => {
                    self.searching = false;
                    self.query.clear();
                }
                Key::Enter => self.searching = false,
                Key::Backspace => {
                    self.query.pop();
                }
                Key::Char(c) => self.query.push(c),
                Key::Space => self.query.push(' '),
                _ => {}
            }
            self.sel[0] = 0;
            return Action::None;
        }
        let n = self.count();
        let t = self.tab_index();
        let s = &mut self.sel[t];
        match key {
            Key::Char('q') | Key::Escape => return Action::Close,
            Key::Char('?') => self.help = true,
            Key::Char(c @ '1'..='4') => self.tab = TABS[c as usize - '1' as usize],
            Key::Tab => self.tab = TABS[(t + 1) % 4],
            Key::BackTab => self.tab = TABS[(t + 3) % 4],
            Key::Down | Key::Char('j') => {
                *s = (*s + 1).min(n.saturating_sub(1));
                self.follow = false;
            }
            Key::Up | Key::Char('k') => {
                *s = s.saturating_sub(1);
                self.follow = false;
            }
            Key::Char('d') => *s = (*s + 20).min(n.saturating_sub(1)),
            Key::Char('u') => *s = s.saturating_sub(20),
            Key::Char('g') => {
                *s = 0;
                self.follow = false;
            }
            Key::Char('G') => {
                *s = n.saturating_sub(1);
                self.follow = self.live;
            }
            Key::Char('/') => {
                self.searching = true;
                self.tab = Tab::Packets;
            }
            Key::Char('i') => {
                self.show_idle = !self.show_idle;
                self.note = Some((if self.show_idle { "idle packets shown" } else { "idle packets hidden" }.into(), false));
            }
            Key::Char(']') => {
                if self.tab == Tab::Packets {
                    let lines = self.lines();
                    if let Some(next) = (self.sel[0] + 1..lines.len()).find(|&i| matches!(lines[i], Line::Gap { .. })) {
                        self.sel[0] = next;
                    } else {
                        self.note = Some(("no more gaps".into(), false));
                    }
                }
            }
            Key::Char('[') => {
                if self.tab == Tab::Packets {
                    let lines = self.lines();
                    if let Some(prev) = (0..self.sel[0]).rev().find(|&i| matches!(lines[i], Line::Gap { .. })) {
                        self.sel[0] = prev;
                    }
                }
            }
            Key::Enter => {
                if let Some(r) = self.selected_row() {
                    return Action::Inspect(r);
                }
            }
            Key::Char('x') => {
                if let Some(r) = self.selected_row() {
                    return Action::Hex(self.rows[r].offset);
                }
            }
            Key::Char('f') => {
                if let Some(r) = self.selected_row() {
                    if self.rows[r].unit.is_some() {
                        return Action::Unit(r);
                    }
                    self.note = Some(("this packet didn't come in a frame or a CLTU".into(), true));
                }
            }
            Key::Char('t') => return Action::PickParameter,
            Key::Char('w') => self.menu = Some(0),
            Key::Char('F') if !self.live => return Action::Reframe,
            _ => {}
        }
        Action::None
    }

    fn write_items(&self) -> Vec<(String, Write)> {
        let mut items = vec![("the packets shown, as binary".to_string(), Write::Binary), ("the packets shown, as hex lines".to_string(), Write::Hex)];
        if let Some((name, _)) = &self.param {
            items.push((format!("{name}'s samples, as CSV"), Write::Csv));
        }
        items
    }

    /// The rows the packets tab shows.
    pub fn shown(&self) -> Vec<usize> {
        self.lines().into_iter().filter_map(|l| if let Line::Row(i) = l { Some(i) } else { None }).collect()
    }

    /// New rows arrived: follow them when following.
    pub fn arrived(&mut self) {
        if self.follow && self.tab == Tab::Packets {
            self.sel[0] = self.lines().len().saturating_sub(1);
        }
    }
}

/// Eight-level bars across the samples.
fn curve(samples: &[Sample], width: usize) -> String {
    let values: Vec<f64> = samples.iter().filter_map(|s| s.number).collect();
    if values.len() < 2 || width == 0 {
        return String::new();
    }
    let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    let bars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let n = width.min(values.len());
    (0..n)
        .map(|i| {
            let v = values[i * values.len() / n];
            if hi > lo { bars[(((v - lo) / (hi - lo)) * 7.0).round() as usize] } else { bars[3] }
        })
        .collect()
}

pub fn title(p: &StreamPage) -> String {
    format!("*{}: {}*", if p.live { "live" } else { "recording" }, p.title)
}

pub fn layout(p: &StreamPage, cols: usize) -> Page {
    let (left, width) = frame(cols, 190);
    let mut g = Grid::new();
    let x = g.put(1, left, &p.title, Role::Title) + 2;
    g.put(1, x, &p.framing, Role::Muted);
    let packets = p.rows.iter().filter(|r| !r.idle).count();
    let gaps = p.lines().iter().filter(|l| matches!(l, Line::Gap { .. })).count();
    let bad = p.rows.iter().filter(|r| r.bad.is_some()).count();
    let plural = |n: usize, one: &str| format!("{} {one}{}", thousands(n), if n == 1 { "" } else { "s" });
    let mut right = format!("{} · {} · {bad} bad", plural(packets, "packet"), plural(gaps, "gap"));
    if !p.done && !p.live {
        right.push_str(" · reading…");
    }
    let rrole = if let Some((text, ok)) = &p.status {
        right = format!("● {text} · {right}");
        if *ok { Role::Good } else { Role::Warn }
    } else {
        Role::Muted
    };
    g.put(1, (left + width).saturating_sub(right.chars().count()), &right, rrole);
    let search = if p.searching { format!("/ {}▏", p.query) } else if p.query.is_empty() { "/ filter: words, apid: type: stype: spid: vc: seq: check:bad".to_string() } else { format!("/ {}  (Esc clears)", p.query) };
    let e = g.put(2, left, &fit_tail(&search, width), if p.searching { Role::Title } else { Role::Muted });
    if p.searching {
        g.panels.push((2, left..e.max(left + 40)));
    }
    let names = ["Packets", "By APID", "Parameter", "Problems"];
    let counts = [p.shown().len(), p.stats().len(), p.param.as_ref().map(|(_, s)| s.len()).unwrap_or(0), p.problems().len()];
    let mut x = left;
    for (i, name) in names.iter().enumerate() {
        x = g.put(4, x, &(i + 1).to_string(), Role::Accent) + 1;
        let on = TABS[i] == p.tab;
        let end = g.put(4, x, name, if on { Role::Title } else { Role::Muted });
        if on {
            g.panels.push((4, x..end));
        }
        x = g.put(4, end + 1, &thousands(counts[i]), Role::Muted) + 3;
    }
    let idle = if p.show_idle { "idle shown · i" } else { "idle hidden · i" };
    g.put(4, (left + width).saturating_sub(idle.len()), idle, Role::Muted);
    let mut y = 5;
    if let Some((text, bad)) = &p.note {
        g.put(y, left, &fit(text, width), if *bad { Role::Bad } else { Role::Good });
        y += 1;
    }
    g.rule(y, left..left + width);
    y += 1;
    let sel = p.sel[p.tab_index()];
    let mut anchor = y;
    let window = |n: usize| {
        let start = sel.saturating_sub(150).min(n.saturating_sub(300));
        start..(start + 300).min(n)
    };
    match p.tab {
        Tab::Packets => {
            let cols = [(0, "TIME"), (24, "APID"), (31, "SPID · NAME"), (58, "PUS"), (66, "SEQ"), (73, "LEN"), (79, "CHECK")];
            for (dx, h) in cols {
                g.put(y, left + dx, h, Role::Muted);
            }
            y += 1;
            let lines = p.lines();
            if lines.is_empty() {
                g.put(y, left, if p.done || p.live { "Nothing here -- Esc clears the filter, i shows idle packets." } else { "Reading…" }, Role::Muted);
            }
            let range = window(lines.len());
            for (k, line) in lines[range.clone()].iter().enumerate() {
                let i = range.start + k;
                match line {
                    Line::Gap { apid, from, to, missing } => {
                        g.put(y, left + 2, &fit(&format!("⚠ {missing} missing on APID 0x{apid:03X} -- sequence {from} to {to}"), width - 2), Role::Warn);
                    }
                    Line::Row(r) => {
                        let r = &p.rows[*r];
                        g.put(y, left, &fit(r.time.as_deref().unwrap_or("-"), 23), Role::Muted);
                        g.put(y, left + 24, &format!("0x{:03X}", r.apid), Role::Text);
                        let name = match &r.spid {
                            Some(s) => format!("{s} {}", r.label),
                            None => r.label.clone(),
                        };
                        g.put(y, left + 31, &fit(&name, 26), if r.spid.is_some() || r.is_tc { Role::Title } else { Role::Text });
                        g.put(y, left + 58, &r.pus.map(|(a, b)| format!("{a},{b}")).unwrap_or_default(), Role::Muted);
                        g.put(y, left + 66, &r.seq.to_string(), Role::Muted);
                        g.put(y, left + 73, &r.len.to_string(), Role::Muted);
                        let (check, role) = match (&r.bad, &r.verifies) {
                            (Some(why), _) => (why.clone(), Role::Bad),
                            (None, Some(v)) => (format!("ok · {v}"), Role::Good),
                            (None, None) => ("ok".to_string(), Role::Good),
                        };
                        g.put(y, left + 79, &fit(&check, width.saturating_sub(79)), role);
                    }
                }
                if i == sel {
                    g.focus(y, left..left + width);
                    anchor = y;
                }
                y += 1;
            }
        }
        Tab::Apids => {
            let cols = [(0, "APID"), (8, "PACKETS"), (18, "GAPS"), (26, "MISSING"), (36, "BAD"), (42, "FIRST"), (66, "LAST"), (90, "SPIDS")];
            for (dx, h) in cols {
                g.put(y, left + dx, h, Role::Muted);
            }
            y += 1;
            for (i, (apid, s)) in p.stats().iter().enumerate() {
                g.put(y, left, &format!("0x{apid:03X}"), Role::Title);
                g.put(y, left + 8, &thousands(s.packets), Role::Text);
                g.put(y, left + 18, &s.gaps.to_string(), if s.gaps > 0 { Role::Warn } else { Role::Muted });
                g.put(y, left + 26, &s.missing.to_string(), if s.missing > 0 { Role::Warn } else { Role::Muted });
                g.put(y, left + 36, &s.bad.to_string(), if s.bad > 0 { Role::Bad } else { Role::Muted });
                g.put(y, left + 42, &fit(s.first.as_deref().unwrap_or("-"), 23), Role::Muted);
                g.put(y, left + 66, &fit(s.last.as_deref().unwrap_or("-"), 23), Role::Muted);
                g.put(y, left + 90, &fit(&s.spids.join(" "), width.saturating_sub(90)), Role::Text);
                if i == sel {
                    g.focus(y, left..left + width);
                    anchor = y;
                }
                y += 1;
            }
        }
        Tab::Parameter => match &p.param {
            None => {
                g.put(y, left, "t picks a TM parameter to follow through the packets.", Role::Muted);
            }
            Some((name, samples)) => {
                let nums: Vec<f64> = samples.iter().filter_map(|s| s.number).collect();
                let summary = if nums.is_empty() {
                    format!("{name} · {} samples", samples.len())
                } else {
                    let (lo, hi) = nums.iter().fold((f64::MAX, f64::MIN), |(l, h), &v| (l.min(v), h.max(v)));
                    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                    format!("{name} · {} samples · min {lo:.3} · max {hi:.3} · mean {mean:.3}", samples.len())
                };
                g.put(y, left, &summary, Role::Title);
                y += 1;
                let c = curve(samples, width.saturating_sub(8));
                if !c.is_empty() {
                    g.put(y, left, "curve", Role::Muted);
                    g.put(y, left + 8, &c, Role::Good);
                    y += 1;
                }
                y += 1;
                for (dx, h) in [(0, "TIME"), (24, "RAW"), (40, "VALUE"), (64, "LIMITS")] {
                    g.put(y, left + dx, h, Role::Muted);
                }
                y += 1;
                let range = window(samples.len());
                for (k, s) in samples[range.clone()].iter().enumerate() {
                    let i = range.start + k;
                    g.put(y, left, &fit(s.time.as_deref().unwrap_or("-"), 23), Role::Muted);
                    g.put(y, left + 24, &fit(&s.raw, 15), Role::Muted);
                    g.put(y, left + 40, &fit(&s.value, 23), Role::Text);
                    match &s.off {
                        Some((why, hard)) => g.put(y, left + 64, &fit(why, width.saturating_sub(64)), if *hard { Role::Bad } else { Role::Warn }),
                        None => g.put(y, left + 64, "within", Role::Good),
                    };
                    if i == sel {
                        g.focus(y, left..left + width);
                        anchor = y;
                    }
                    y += 1;
                }
            }
        },
        Tab::Problems => {
            let problems = p.problems();
            if problems.is_empty() {
                g.put(y, left, "No problems found.", Role::Good);
            }
            for (i, (text, _)) in problems.iter().enumerate() {
                g.put(y, left, &fit(text, width), Role::Text);
                if i == sel {
                    g.focus(y, left..left + width);
                    anchor = y;
                }
                y += 1;
            }
        }
    }
    if let Some(at) = p.menu {
        let mut rows = vec![vec![("Write out".to_string(), Role::Title)], Vec::new()];
        for (i, (label, _)) in p.write_items().iter().enumerate() {
            let on = i == at;
            rows.push(vec![(format!("{}{label}", if on { "› " } else { "  " }), if on { Role::Title } else { Role::Text })]);
        }
        rows.push(Vec::new());
        rows.push(vec![("Enter writes it beside the recording · Esc leaves it".into(), Role::Muted)]);
        g.popup = Some(Popup { line: anchor, col: left + 4, rows });
    } else if p.help {
        let rows: Vec<Vec<(String, Role)>> = [
            ("1-4 Tab", "packets, by APID, a parameter, problems"),
            ("j k g G d u", "move; G follows a live source"),
            ("] [", "next, previous gap"),
            ("/", "filter: words, apid: type: stype: spid: vc: seq: check:bad"),
            ("i", "idle packets shown, hidden"),
            ("Enter x", "inspect the packet, its bytes in hex"),
            ("f", "inspect the CADU or CLTU it came in"),
            ("t", "follow a TM parameter"),
            ("w", "write the packets shown, or the samples"),
            ("F", "read again with the next framing"),
            ("q", "close"),
        ]
        .iter()
        .map(|(k, w)| vec![(format!("{k:<12}"), Role::Accent), (w.to_string(), Role::Text)])
        .collect();
        let mut all = vec![vec![("Keys".to_string(), Role::Title)], Vec::new()];
        all.extend(rows);
        g.popup = Some(Popup { line: anchor, col: left + 4, rows: all });
    }
    g.keys(left, width, &[("1-4", "tabs"), ("Enter", "inspect"), ("f", "its frame"), ("/", "filter"), ("] [", "gaps"), ("t", "parameter"), ("w", "write"), ("x", "hex"), ("?", "all keys"), ("q", "close")]);
    g.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(apid: u16, seq: u16, bad: bool) -> Row {
        Row { apid, seq, len: 29, label: "TCS_HK_FAST".into(), spid: Some("30211".into()), pus: Some((3, 25)), time: Some(format!("14:32:{seq:02}")), bad: bad.then(|| "CRC 60E5, computed 61E4".into()), ..Default::default() }
    }

    fn page() -> StreamPage {
        let mut p = StreamPage::new(MibKey::default(), "bench.bin".into(), false);
        p.rows = vec![row(0x3F2, 289, false), row(0x3F2, 290, false), row(0x3F2, 291, false), row(0x3F2, 295, true), row(0x100, 7, false), Row { apid: 0x7FF, idle: true, label: "idle".into(), ..Default::default() }];
        p.done = true;
        p
    }

    #[test]
    fn gaps_bad_checks_and_idle_packets() {
        let mut p = page();
        let text = layout(&p, 160).text;
        assert!(text.contains("⚠ 3 missing on APID 0x3F2 -- sequence 292 to 294"), "{text}");
        assert!(text.contains("CRC 60E5, computed 61E4"));
        assert!(!text.contains("0x7FF"), "idle packets hidden: {text}");
        assert!(text.contains("5 packets · 1 gap · 1 bad"), "{text}");
        p.key(Key::Char(']'));
        assert!(matches!(p.lines()[p.sel[0]], Line::Gap { .. }));
        p.key(Key::Char('i'));
        assert!(p.lines().iter().any(|l| matches!(l, Line::Row(5))));
    }

    #[test]
    fn filters_tabs_and_problems() {
        let mut p = page();
        p.key(Key::Char('/'));
        for c in "apid:0x100".chars() {
            p.key(Key::Char(c));
        }
        p.key(Key::Enter);
        assert_eq!(p.shown(), vec![4]);
        p.key(Key::Char('/'));
        p.key(Key::Escape);
        p.key(Key::Char('2'));
        let text = layout(&p, 160).text;
        assert!(text.contains("0x3F2") && text.contains("0x100"), "{text}");
        p.key(Key::Char('4'));
        let problems = p.problems();
        assert!(problems.iter().any(|(t, _)| t.contains("3 missing")));
        assert!(problems.iter().any(|(t, r)| t.contains("CRC") && *r == Some(3)));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), Action::Inspect(3));
        p.key(Key::Char('w'));
        assert_eq!(p.key(Key::Enter), Action::Write(Write::Binary));
    }

    #[test]
    fn a_parameters_samples_and_curve() {
        let mut p = page();
        p.param = Some((
            "NTH00123".into(),
            (0..10).map(|i| Sample { row: 0, time: None, raw: i.to_string(), value: format!("{i}.0 degC"), number: Some(i as f64), off: (i > 8).then(|| ("soft limit -10..8".to_string(), false)) }).collect(),
        ));
        p.key(Key::Char('3'));
        let text = layout(&p, 160).text;
        assert!(text.contains("NTH00123 · 10 samples · min 0.000 · max 9.000"), "{text}");
        assert!(text.contains('▁') && text.contains('█'));
        assert!(p.problems().iter().any(|(t, _)| t.contains("soft limit")));
        p.key(Key::Char('w'));
        p.key(Key::Char('j'));
        p.key(Key::Char('j'));
        assert_eq!(p.key(Key::Enter), Action::Write(Write::Csv));
    }
}
