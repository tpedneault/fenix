//! The MIBs one project uses, loaded once and joined: every definition
//! of the five kinds the MIB page lists (telecommands, TC parameters,
//! TM packets, TM parameters, calibrations) as an `Entry`, indexed by
//! name, with what each one is used by and every problem found along
//! the way -- files that couldn't be read, and references that point at
//! nothing. Built off the UI thread (`MibSet::load`) and swapped in
//! whole; nothing here changes after that.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::parse::Problem;
use crate::row::Row;
use crate::{schema, types, MibIndex, MibRoot};

/// The kinds of definition the MIB page has a tab for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Telecommand,
    TcParam,
    TmPacket,
    TmParam,
    Calibration,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Telecommand, Kind::TcParam, Kind::TmPacket, Kind::TmParam, Kind::Calibration];

    fn slot(self) -> usize {
        self as usize
    }

    /// The tab's name.
    pub fn plural(self) -> &'static str {
        match self {
            Kind::Telecommand => "Telecommands",
            Kind::TcParam => "TC parameters",
            Kind::TmPacket => "TM packets",
            Kind::TmParam => "TM parameters",
            Kind::Calibration => "Calibrations",
        }
    }

    pub fn singular(self) -> &'static str {
        match self {
            Kind::Telecommand => "telecommand",
            Kind::TcParam => "TC parameter",
            Kind::TmPacket => "TM packet",
            Kind::TmParam => "TM parameter",
            Kind::Calibration => "calibration",
        }
    }

    /// The short tag shown before a name.
    pub fn tag(self) -> &'static str {
        match self {
            Kind::Telecommand => "TC",
            Kind::TcParam => "PAR",
            Kind::TmPacket => "PKT",
            Kind::TmParam => "TM",
            Kind::Calibration => "CAL",
        }
    }

    /// The list's column headings: the name, `Entry::cols` in order, then
    /// the description.
    pub fn headers(self) -> &'static [&'static str] {
        match self {
            Kind::Telecommand => &["Name", "PUS", "APID", "Pars"],
            Kind::TcParam => &["Name", "Type", "Unit", "Cal"],
            Kind::TmPacket => &["SPID", "Name", "PUS", "APID"],
            Kind::TmParam => &["Name", "Type", "Unit", "Cal"],
            Kind::Calibration => &["Id", "Kind", "Values", "Unit"],
        }
    }
}

/// The kinds of calibration, each its own table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalKind {
    /// `CAF`/`CAP`: a TM numeric curve.
    Numeric,
    /// `CCA`/`CCS`: a TC numeric curve.
    TcNumeric,
    /// `TXF`/`TXP`: TM raw ranges to text.
    Text,
    /// `PAF`/`PAS`: TC text to raw values.
    Status,
    /// `PRF`/`PRV`: a TC range check.
    Range,
    /// `MCF`: a polynomial.
    Polynomial,
    /// `LGF`: a logarithmic calibration.
    Logarithmic,
}

impl CalKind {
    pub const ALL: [CalKind; 7] =
        [CalKind::Numeric, CalKind::TcNumeric, CalKind::Text, CalKind::Status, CalKind::Range, CalKind::Polynomial, CalKind::Logarithmic];

    pub fn table(self) -> &'static str {
        match self {
            CalKind::Numeric => "caf",
            CalKind::TcNumeric => "cca",
            CalKind::Text => "txf",
            CalKind::Status => "paf",
            CalKind::Range => "prf",
            CalKind::Polynomial => "mcf",
            CalKind::Logarithmic => "lgf",
        }
    }

    /// The column holding its id.
    pub fn id_field(self) -> &'static str {
        match self {
            CalKind::Numeric => "CAF_NUMBR",
            CalKind::TcNumeric => "CCA_NUMBR",
            CalKind::Text => "TXF_NUMBR",
            CalKind::Status => "PAF_NUMBR",
            CalKind::Range => "PRF_NUMBR",
            CalKind::Polynomial => "MCF_IDENT",
            CalKind::Logarithmic => "LGF_IDENT",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CalKind::Numeric | CalKind::TcNumeric => "numeric",
            CalKind::Text | CalKind::Status => "status",
            CalKind::Range => "range",
            CalKind::Polynomial => "polynomial",
            CalKind::Logarithmic => "log",
        }
    }
}

/// One definition in a set: `set.entries(kind)[index]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefRef {
    pub kind: Kind,
    pub index: usize,
}

/// One definition, as the list shows it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: Kind,
    /// Its name: a mnemonic, a SPID, a calibration's id.
    pub name: String,
    /// Another name it goes by: a TM packet's `TPCF_NAME`.
    pub alias: String,
    pub description: String,
    /// Which of the set's MIBs it's from.
    pub root: usize,
    /// The row that defines it.
    pub row: Row,
    pub cal: Option<CalKind>,
    /// The columns between the name and the description (`Kind::headers`
    /// after "Name"). An APID column holds the MIB's text; the page
    /// formats it.
    pub cols: Vec<String>,
    /// Other values a search looks in, each with what it is: a status
    /// text, a unit, a second description.
    pub extras: Vec<(&'static str, String)>,
}

/// When each table file of a set's MIBs last changed, to tell when the
/// set is out of date.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamp(Vec<(PathBuf, Option<(SystemTime, u64)>)>);

impl Stamp {
    pub fn of(roots: &[MibRoot]) -> Stamp {
        let mut files = Vec::new();
        for root in roots {
            if root.path.is_file() {
                let meta = std::fs::metadata(&root.path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
                files.push((root.path.clone(), meta));
                continue;
            }
            for table in schema::all_tables() {
                let path = root.path.join(format!("{table}.dat"));
                let meta = std::fs::metadata(&path).ok().and_then(|m| Some((m.modified().ok()?, m.len())));
                files.push((path, meta));
            }
        }
        Stamp(files)
    }
}

/// What one of the set's MIBs holds, for its line in the settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootInfo {
    /// `VDF_NAME` and `VDF_COMMENT`, when it has a `vdf.dat`.
    pub version: Option<String>,
    pub tables: usize,
    pub rows: usize,
}

/// The MIBs of one project, loaded and joined.
pub struct MibSet {
    index: MibIndex,
    entries: [Vec<Entry>; 5],
    names: HashMap<(Kind, usize, String), usize>,
    cals: HashMap<(usize, &'static str, String), usize>,
    packet_names: HashMap<(usize, String), usize>,
    used_by: HashMap<DefRef, Vec<DefRef>>,
    problems: Vec<Problem>,
    default_root: usize,
    info: Vec<RootInfo>,
    stamp: Stamp,
}

impl std::fmt::Debug for MibSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sizes: Vec<usize> = self.entries.iter().map(Vec::len).collect();
        f.debug_struct("MibSet").field("roots", &self.roots()).field("entries", &sizes).field("problems", &self.problems.len()).finish()
    }
}

type Groups<'a> = HashMap<(usize, String), Vec<&'a Row>>;

/// `table`'s rows, grouped by `field` within each root.
fn group<'a>(index: &'a MibIndex, table: &str, field: &str) -> Groups<'a> {
    let mut out: Groups = HashMap::new();
    for row in index.rows(table) {
        out.entry((row.source.root_index, row.clean(field).to_string())).or_default().push(row);
    }
    out
}

fn rows_of<'a>(groups: &'a Groups, root: usize, key: &str) -> &'a [&'a Row] {
    groups.get(&(root, key.to_string())).map(Vec::as_slice).unwrap_or(&[])
}

fn push_unique(list: &mut Vec<DefRef>, def: DefRef) {
    if !list.contains(&def) {
        list.push(def);
    }
}

impl MibSet {
    /// Reads and joins every MIB in `roots`. `default` names the one
    /// that wins a name defined in several; the first when it's `None`
    /// or names none of them. Never fails: what's wrong is in
    /// `problems`.
    pub fn load(roots: Vec<MibRoot>, default: Option<&str>) -> MibSet {
        let stamp = Stamp::of(&roots);
        let default_root = default.and_then(|d| roots.iter().position(|r| r.label == d)).unwrap_or(0);
        let mut index = MibIndex::new(roots);
        index.refresh();
        let mut set = MibSet {
            entries: Default::default(),
            names: HashMap::new(),
            cals: HashMap::new(),
            packet_names: HashMap::new(),
            used_by: HashMap::new(),
            problems: index.problems().to_vec(),
            default_root,
            info: Vec::new(),
            stamp,
            index: MibIndex::new(Vec::new()),
        };
        set.build(&index);
        set.index = index;
        set.problems.sort_by(|a, b| (a.root, &a.file, a.line).cmp(&(b.root, &b.file, b.line)));
        set
    }

    fn add(&mut self, entry: Entry) -> DefRef {
        let kind = entry.kind;
        let list = &mut self.entries[kind.slot()];
        let index = list.len();
        match (kind, entry.cal) {
            (Kind::Calibration, Some(cal)) => {
                self.cals.entry((entry.root, cal.table(), entry.name.clone())).or_insert(index);
            }
            _ => {
                self.names.entry((kind, entry.root, entry.name.clone())).or_insert(index);
            }
        }
        if kind == Kind::TmPacket && !entry.alias.is_empty() {
            self.packet_names.entry((entry.root, entry.alias.clone())).or_insert(index);
        }
        list.push(entry);
        DefRef { kind, index }
    }

    fn problem(&mut self, row: &Row, message: String) {
        self.problems.push(Problem { root: row.source.root_index, file: row.source.file.clone(), line: Some(row.source.line), message });
    }

    fn build(&mut self, index: &MibIndex) {
        let cdf = group(index, "cdf", "CDF_CNAME");
        let pas = group(index, "pas", "PAS_NUMBR");
        let prv = group(index, "prv", "PRV_NUMBR");
        let txp = group(index, "txp", "TXP_NUMBR");
        let cap = group(index, "cap", "CAP_NUMBR");
        let ccs = group(index, "ccs", "CCS_NUMBR");
        let tpcf = group(index, "tpcf", "TPCF_SPID");
        let entry = |kind: Kind, row: &Row, name: &str, description: &str| Entry {
            kind,
            name: name.to_string(),
            alias: String::new(),
            description: description.to_string(),
            root: row.source.root_index,
            row: row.clone(),
            cal: None,
            cols: Vec::new(),
            extras: Vec::new(),
        };
        let texts = |rows: &[&Row], field: &str| -> Vec<String> {
            rows.iter().map(|r| r.clean(field).to_string()).filter(|t| !t.is_empty()).collect()
        };

        // Calibrations first: the parameters' columns name their kind.
        for cal in CalKind::ALL {
            for row in index.rows(cal.table()) {
                let root = row.source.root_index;
                let id = row.clean(cal.id_field());
                let prefix = cal.table().to_uppercase();
                let mut e = entry(Kind::Calibration, row, id, row.clean(&format!("{prefix}_DESCR")));
                e.cal = Some(cal);
                let (values, aliases) = match cal {
                    CalKind::Numeric => (format!("{} points", rows_of(&cap, root, id).len()), Vec::new()),
                    CalKind::TcNumeric => (format!("{} points", rows_of(&ccs, root, id).len()), Vec::new()),
                    CalKind::Text => {
                        let a = texts(rows_of(&txp, root, id), "TXP_ALTXT");
                        (a.join(" "), a)
                    }
                    CalKind::Status => {
                        let a = texts(rows_of(&pas, root, id), "PAS_ALTXT");
                        (a.join(" "), a)
                    }
                    CalKind::Range => {
                        let r = rows_of(&prv, root, id);
                        let text = r.iter().map(|r| format!("{}..{}", r.clean("PRV_MINVAL"), r.clean("PRV_MAXVAL"))).collect::<Vec<_>>().join(", ");
                        (text, Vec::new())
                    }
                    CalKind::Polynomial | CalKind::Logarithmic => {
                        let terms: Vec<String> = (1..=5).map(|i| row.clean(&format!("{prefix}_POL{i}")).to_string()).filter(|t| !t.is_empty()).collect();
                        (terms.join(" "), Vec::new())
                    }
                };
                let unit = match cal {
                    CalKind::Numeric => row.clean("CAF_UNIT"),
                    CalKind::TcNumeric => row.clean("CCA_UNIT"),
                    CalKind::Range => row.clean("PRF_UNIT"),
                    _ => "",
                };
                e.cols = vec![cal.label().to_string(), values, unit.to_string()];
                e.extras = aliases.into_iter().map(|a| ("status", a)).collect();
                if !unit.is_empty() {
                    e.extras.push(("unit", unit.to_string()));
                }
                self.add(e);
            }
        }

        // TC parameters.
        for row in index.rows("cpc") {
            let root = row.source.root_index;
            let mut e = entry(Kind::TcParam, row, row.clean("CPC_NAME"), row.clean("CPC_DESCR"));
            let cal = if !row.clean("CPC_PAFREF").is_empty() {
                "status"
            } else if !row.clean("CPC_PRFREF").is_empty() {
                "range"
            } else if !row.clean("CPC_CCAREF").is_empty() {
                "numeric"
            } else {
                ""
            };
            e.cols = vec![types::decode(row.clean("CPC_PTC"), row.clean("CPC_PFC")).name, row.clean("CPC_UNIT").to_string(), cal.to_string()];
            e.extras = texts(rows_of(&pas, root, row.clean("CPC_PAFREF")), "PAS_ALTXT").into_iter().map(|a| ("status", a)).collect();
            for (what, field) in [("unit", "CPC_UNIT"), ("description", "CPC_DESCR2")] {
                if !row.clean(field).is_empty() {
                    e.extras.push((what, row.clean(field).to_string()));
                }
            }
            let this = self.add(e);
            for (field, cal) in [("CPC_CCAREF", CalKind::TcNumeric), ("CPC_PAFREF", CalKind::Status), ("CPC_PRFREF", CalKind::Range)] {
                let id = row.clean(field);
                if id.is_empty() {
                    continue;
                }
                match self.find_cal(root, cal, id) {
                    Some(target) => push_unique(self.used_by.entry(target).or_default(), this),
                    None => self.problem(row, format!("{} -> {} {id} isn't in {}.dat", row.clean("CPC_NAME"), cal.label(), cal.table())),
                }
            }
        }

        // Telecommands, and the parameters they use.
        for row in index.rows("ccf") {
            let root = row.source.root_index;
            let name = row.clean("CCF_CNAME");
            let params = rows_of(&cdf, root, name);
            let mut e = entry(Kind::Telecommand, row, name, row.clean("CCF_DESCR"));
            e.cols = vec![
                format!("{},{}", row.clean("CCF_TYPE"), row.clean("CCF_STYPE")),
                row.clean("CCF_APID").to_string(),
                params.iter().filter(|p| p.clean("CDF_VALUE").is_empty() && !p.clean("CDF_PNAME").is_empty()).count().to_string(),
            ];
            for (what, field) in [("subsystem", "CCF_SUBSYS"), ("description", "CCF_DESCR2")] {
                if !row.clean(field).is_empty() {
                    e.extras.push((what, row.clean(field).to_string()));
                }
            }
            let this = self.add(e);
            for p in params {
                let pname = p.clean("CDF_PNAME");
                if pname.is_empty() {
                    continue;
                }
                match self.find(Kind::TcParam, root, pname) {
                    Some(target) => push_unique(self.used_by.entry(target).or_default(), this),
                    None => self.problem(p, format!("{name} -> {pname} has no CPC row")),
                }
            }
        }
        for row in index.rows("cdf") {
            let name = row.clean("CDF_CNAME");
            if self.find(Kind::Telecommand, row.source.root_index, name).is_none() {
                self.problem(row, format!("{name} isn't in ccf.dat"));
            }
        }

        // TM packets.
        for row in index.rows("pid") {
            let root = row.source.root_index;
            let spid = row.clean("PID_SPID");
            let mut e = entry(Kind::TmPacket, row, spid, row.clean("PID_DESCR"));
            let tp = rows_of(&tpcf, root, spid).first();
            e.alias = tp.map(|t| t.clean("TPCF_NAME").to_string()).unwrap_or_default();
            e.cols = vec![e.alias.clone(), format!("{},{}", row.clean("PID_TYPE"), row.clean("PID_STYPE")), row.clean("PID_APID").to_string()];
            self.add(e);
        }

        // TM parameters, their calibrations, and the packets carrying them.
        for row in index.rows("pcf") {
            let root = row.source.root_index;
            let categ = row.clean("PCF_CATEG");
            let curtx = row.clean("PCF_CURTX");
            let cal = match (categ, curtx.is_empty()) {
                (_, true) => None,
                ("S", _) => self.find_cal(root, CalKind::Text, curtx).map(|c| (c, CalKind::Text)),
                _ => [CalKind::Numeric, CalKind::Polynomial, CalKind::Logarithmic].into_iter().find_map(|k| self.find_cal(root, k, curtx).map(|c| (c, k))),
            };
            let mut e = entry(Kind::TmParam, row, row.clean("PCF_NAME"), row.clean("PCF_DESCR"));
            e.cols = vec![
                types::decode(row.clean("PCF_PTC"), row.clean("PCF_PFC")).name,
                row.clean("PCF_UNIT").to_string(),
                cal.map(|(_, k)| k.label()).unwrap_or("").to_string(),
            ];
            if let Some((c, CalKind::Text)) = cal {
                let id = self.get(c).name.clone();
                e.extras = texts(rows_of(&txp, root, &id), "TXP_ALTXT").into_iter().map(|a| ("status", a)).collect();
            }
            for (what, field) in [("unit", "PCF_UNIT"), ("subsystem", "PCF_SUBSYS"), ("description", "PCF_DESCR2")] {
                if !row.clean(field).is_empty() {
                    e.extras.push((what, row.clean(field).to_string()));
                }
            }
            let this = self.add(e);
            match cal {
                Some((c, _)) => push_unique(self.used_by.entry(c).or_default(), this),
                None if !curtx.is_empty() => self.problem(row, format!("{} -> calibration {curtx} isn't in the MIB", row.clean("PCF_NAME"))),
                None => {}
            }
        }
        for row in index.rows("plf") {
            let root = row.source.root_index;
            let (name, spid) = (row.clean("PLF_NAME"), row.clean("PLF_SPID"));
            match (self.find(Kind::TmParam, root, name), self.find(Kind::TmPacket, root, spid)) {
                (Some(param), Some(packet)) => push_unique(self.used_by.entry(param).or_default(), packet),
                (None, _) => self.problem(row, format!("{spid} -> {name} has no PCF row")),
                (_, None) => self.problem(row, format!("{name} is placed in SPID {spid}, which isn't in pid.dat")),
            }
        }
        for row in index.rows("css") {
            let elem = row.clean("CSS_ELEMID");
            if row.clean("CSS_TYPE") == "C" && self.find(Kind::Telecommand, row.source.root_index, elem).is_none() {
                self.problem(row, format!("{} sends {elem}, which isn't in ccf.dat", row.clean("CSS_SQNAME")));
            }
        }

        for (i, _) in index.roots().iter().enumerate() {
            let mut info = RootInfo::default();
            for table in schema::all_tables() {
                let n = index.rows(table).iter().filter(|r| r.source.root_index == i).count();
                if n > 0 {
                    info.tables += 1;
                    info.rows += n;
                }
            }
            info.version = index.rows("vdf").iter().find(|r| r.source.root_index == i).map(|r| {
                let (name, comment) = (r.clean("VDF_NAME"), r.clean("VDF_COMMENT"));
                if comment.is_empty() { name.to_string() } else { format!("{name} {comment}") }
            });
            self.info.push(info);
        }
    }

    /// The raw rows, for what the entries don't carry.
    pub fn index(&self) -> &MibIndex {
        &self.index
    }

    pub fn roots(&self) -> &[MibRoot] {
        self.index.roots()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.iter().all(Vec::is_empty)
    }

    pub fn default_root(&self) -> usize {
        self.default_root
    }

    pub fn root_info(&self, root: usize) -> RootInfo {
        self.info.get(root).cloned().unwrap_or_default()
    }

    pub fn root_label(&self, root: usize) -> &str {
        self.roots().get(root).map(|r| r.label.as_str()).unwrap_or("")
    }

    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// More problems, found by checks that need more than the MIB
    /// (`checks::run` with the project's profile), kept in order.
    pub fn add_problems(&mut self, more: Vec<Problem>) {
        self.problems.extend(more);
        self.problems.sort_by(|a, b| (a.root, &a.file, a.line).cmp(&(b.root, &b.file, b.line)));
        self.problems.dedup();
    }

    pub fn stamp(&self) -> &Stamp {
        &self.stamp
    }

    pub fn entries(&self, kind: Kind) -> &[Entry] {
        &self.entries[kind.slot()]
    }

    pub fn get(&self, def: DefRef) -> &Entry {
        &self.entries[def.kind.slot()][def.index]
    }

    /// A definition of `kind` by name in one MIB (a calibration by
    /// `find_cal`; a TM packet by its SPID).
    pub fn find(&self, kind: Kind, root: usize, name: &str) -> Option<DefRef> {
        self.names.get(&(kind, root, name.trim().to_string())).map(|&index| DefRef { kind, index })
    }

    pub fn find_cal(&self, root: usize, cal: CalKind, id: &str) -> Option<DefRef> {
        self.cals.get(&(root, cal.table(), id.trim().to_string())).map(|&index| DefRef { kind: Kind::Calibration, index })
    }

    /// What refers to `def`: the telecommands using a TC parameter, the
    /// parameters using a calibration, the packets carrying a TM
    /// parameter.
    pub fn used_by(&self, def: DefRef) -> &[DefRef] {
        self.used_by.get(&def).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The definition a word in a script names: a telecommand, a TC or
    /// TM parameter, or a TM packet by its name -- from the default MIB
    /// first. SPIDs and calibration ids are plain numbers, so a number
    /// in a script never matches.
    pub fn resolve(&self, word: &str) -> Option<DefRef> {
        let word = word.trim();
        if word.is_empty() {
            return None;
        }
        let order = std::iter::once(self.default_root).chain((0..self.roots().len()).filter(|&r| r != self.default_root));
        for root in order {
            for kind in [Kind::Telecommand, Kind::TcParam, Kind::TmParam] {
                if let Some(def) = self.find(kind, root, word) {
                    return Some(def);
                }
            }
            if let Some(&index) = self.packet_names.get(&(root, word.to_string())) {
                return Some(DefRef { kind: Kind::TmPacket, index });
            }
        }
        None
    }

    /// The sequences that send telecommand `def`: each `(name, its
    /// description, the entry number)`.
    pub fn sequences_using(&self, def: DefRef) -> Vec<(String, String, String)> {
        let e = self.get(def);
        if e.kind != Kind::Telecommand {
            return Vec::new();
        }
        self.index
            .rows("css")
            .iter()
            .filter(|r| r.source.root_index == e.root && r.clean("CSS_TYPE") == "C" && r.clean("CSS_ELEMID") == e.name)
            .map(|r| {
                let name = r.clean("CSS_SQNAME").to_string();
                let descr = self.index.first_row_by_field("csf", "CSF_NAME", &name, Some(e.root)).map(|s| s.clean("CSF_DESC").to_string()).unwrap_or_default();
                (name, descr, r.clean("CSS_ENTRY").to_string())
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) fn temp_root(name: &str) -> MibRoot {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-mib-set-test-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        MibRoot { label: name.to_uppercase(), path: dir }
    }

    pub(crate) fn write(root: &MibRoot, table: &str, contents: &str) {
        std::fs::write(root.path.join(format!("{table}.dat")), contents).unwrap();
    }

    /// Tabs between `fields`, padded with empty fields to `n` columns.
    fn line(fields: &[&str], n: usize) -> String {
        let mut v: Vec<&str> = fields.to_vec();
        v.resize(n.max(fields.len()), "");
        v.join("\t")
    }

    /// A small mission: a heater telecommand with a fixed function id, an
    /// aliased mode and a ranged line number; a housekeeping packet with
    /// a calibrated temperature and a mode status; one sequence.
    pub(crate) fn fixture() -> MibRoot {
        let root = temp_root("fixture");
        write(&root, "vdf", "OPS\tv4.1\n");
        write(&root, "ccf", &format!("{}\n", line(&["ZTC08101", "Set heater control mode", "", "", "N", "", "8", "1", "1010", "3", "", "", "", "", "TCS"], 21)));
        write(
            &root,
            "cdf",
            &[
                line(&["ZTC08101", "F", "Function id", "8", "0", "0", "PTC00001", "", "12"], 10),
                line(&["ZTC08101", "E", "Heater line", "8", "8", "0", "PTH00101"], 10),
                line(&["ZTC08101", "E", "Mode", "8", "16", "0", "PTH00102"], 10),
                line(&["ZTC08101", "E", "Gone", "8", "24", "0", "PTH00990"], 10),
            ]
            .join("\n"),
        );
        write(
            &root,
            "cpc",
            &[
                line(&["PTC00001", "Function id", "3", "4"], 17),
                line(&["PTH00101", "Heater line", "3", "4", "", "D", "", "", "PRF00017"], 17),
                line(&["PTH00102", "Heater control mode", "2", "8", "", "", "", "", "", "", "PAF00042", "", "AUTO"], 17),
            ]
            .join("\n"),
        );
        write(&root, "paf", "PAF00042\tHeater mode\tU\t3\n");
        write(&root, "pas", "PAF00042\tOFF\t0\nPAF00042\tON\t1\nPAF00042\tAUTO\t2\n");
        write(&root, "prf", &line(&["PRF00017", "Heater lines", "", "", "", "1"], 7));
        write(&root, "prv", "PRF00017\t1\t8\n");
        write(&root, "pid", &format!("{}\n", line(&["3", "25", "1010", "0", "0", "30211", "TCS fast housekeeping"], 16)));
        write(&root, "tpcf", "30211\tTCS_HK_FAST\t64\n");
        write(
            &root,
            "pcf",
            &[
                line(&["NTH00123", "Heater 3 temperature", "", "degC", "3", "12", "", "", "", "N", "R", "CAF00310", "", "", "", "", "TCS"], 24),
                line(&["NTH00201", "Heater 1 mode", "", "", "3", "4", "", "", "", "S", "R", "TXF00005"], 24),
            ]
            .join("\n"),
        );
        write(&root, "caf", &line(&["CAF00310", "Thermistor curve", "R", "U", "D", "degC", "2"], 8));
        write(&root, "cap", "CAF00310\t0\t-40\nCAF00310\t4095\t85\n");
        write(&root, "txf", "TXF00005\tHeater mode\tU\t3\n");
        write(&root, "txp", "TXF00005\t0\t0\tOFF\nTXF00005\t1\t1\tON\nTXF00005\t2\t2\tAUTO\n");
        write(&root, "plf", "NTH00123\t30211\t24\t0\t\t\t\t\nNTH00201\t30211\t26\t0\t\t\t\t\nNGONE\t30211\t28\t0\t\t\t\t\n");
        write(&root, "ocp", "NTH00123\t1\tS\t-10\t50\t\t\nNTH00123\t2\tH\t-20\t60\t\t\n");
        write(&root, "csf", "SQTH0010\tHeater checkout\n");
        write(&root, "css", &line(&["SQTH0010", "", "4", "C", "ZTC08101"], 18));
        root
    }

    #[test]
    fn every_kind_is_listed_with_its_columns() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let tc = &set.entries(Kind::Telecommand)[0];
        assert_eq!((tc.name.as_str(), tc.cols.clone()), ("ZTC08101", vec!["8,1".to_string(), "1010".into(), "3".into()]));
        let par = set.get(set.find(Kind::TcParam, 0, "PTH00102").unwrap());
        assert_eq!(par.cols, vec!["enum8".to_string(), "".into(), "status".into()]);
        assert!(par.extras.contains(&("status", "AUTO".to_string())));
        let pkt = &set.entries(Kind::TmPacket)[0];
        assert_eq!((pkt.name.as_str(), pkt.alias.as_str()), ("30211", "TCS_HK_FAST"));
        let tm = set.get(set.find(Kind::TmParam, 0, "NTH00123").unwrap());
        assert_eq!(tm.cols, vec!["uint16".to_string(), "degC".into(), "numeric".into()]);
        assert_eq!(set.entries(Kind::Calibration).len(), 4);
        assert_eq!(set.root_info(0).version.as_deref(), Some("OPS v4.1"));
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn references_run_both_ways() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let tc = set.find(Kind::Telecommand, 0, "ZTC08101").unwrap();
        let mode = set.find(Kind::TcParam, 0, "PTH00102").unwrap();
        assert_eq!(set.used_by(mode), &[tc]);
        let paf = set.find_cal(0, CalKind::Status, "PAF00042").unwrap();
        assert_eq!(set.used_by(paf), &[mode]);
        let temp = set.find(Kind::TmParam, 0, "NTH00123").unwrap();
        let caf = set.find_cal(0, CalKind::Numeric, "CAF00310").unwrap();
        assert_eq!(set.used_by(caf), &[temp]);
        assert_eq!(set.used_by(temp), &[set.find(Kind::TmPacket, 0, "30211").unwrap()]);
        assert_eq!(set.sequences_using(tc), vec![("SQTH0010".to_string(), "Heater checkout".to_string(), "4".to_string())]);
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn dangling_references_are_problems_with_a_line() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let messages: Vec<(String, Option<usize>)> = set.problems().iter().map(|p| (p.message.clone(), p.line)).collect();
        assert!(messages.contains(&("ZTC08101 -> PTH00990 has no CPC row".to_string(), Some(4))), "{messages:?}");
        assert!(messages.iter().any(|(m, l)| m == "30211 -> NGONE has no PCF row" && *l == Some(3)), "{messages:?}");
        assert_eq!(set.problems().len(), 2, "{messages:?}");
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_word_resolves_in_the_default_mib_first() {
        let a = fixture();
        let b = temp_root("other");
        write(&b, "ccf", &format!("{}\n", line(&["ZTC08101", "Same name, other MIB"], 21)));
        let set = MibSet::load(vec![a.clone(), b.clone()], Some("OTHER"));
        let found = set.resolve("ZTC08101").unwrap();
        assert_eq!(set.get(found).description, "Same name, other MIB");
        assert_eq!(set.get(set.resolve("TCS_HK_FAST").unwrap()).name, "30211");
        assert!(set.resolve("30211").is_none());
        assert!(set.resolve("nothing").is_none());
        std::fs::remove_dir_all(&a.path).ok();
        std::fs::remove_dir_all(&b.path).ok();
    }

    #[test]
    fn the_stamp_changes_when_a_table_does() {
        let root = fixture();
        let before = Stamp::of(std::slice::from_ref(&root));
        assert_eq!(before, Stamp::of(std::slice::from_ref(&root)));
        write(&root, "ccf", "ZTC1\tlonger than before, so the size moved\n");
        assert_ne!(before, Stamp::of(std::slice::from_ref(&root)));
        std::fs::remove_dir_all(&root.path).ok();
    }
}
