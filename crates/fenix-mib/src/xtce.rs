//! An XTCE (CCSDS 660.0-B, XTCE 1.0 to 1.2) file read as a MIB: its
//! parameters, packet containers and meta-commands turned into the rows
//! of the MIB tables they correspond to, so the MIB pages, the inspector
//! and the insert form work the same on it:
//!
//! - parameters and their types as `PCF` -- integers, floats,
//!   enumerations, booleans, strings, binary, absolute and relative
//!   times, arrays (one `PCF` for the element, repeated in the packet)
//!   and aggregates (a `PCF` per member, `NAME.member`); a type's
//!   `baseType` is followed;
//! - calibrations as `CAF`/`CAP` (splines), `MCF` (polynomials) and
//!   `TXF`/`TXP` (enumerations) -- the default calibrator, else the
//!   first context one;
//! - a parameter's default static alarm ranges as `OCF`/`OCP` limits
//!   (watch and warning soft, distress, critical and severe hard);
//! - packets: every concrete container that no other one extends, as
//!   `PID`/`TPCF`/`PLF`, identified by the restriction comparisons on it
//!   and the containers it extends, laid out with the entries it
//!   inherits, the containers it includes and the entries it repeats;
//!   a first-identification-field comparison makes a `PIC`;
//! - telecommands as `CCF`/`CDF`, with what their base meta-commands
//!   give them -- arguments, assignments, ancillary data -- their
//!   arguments as `CPC` with `PAF`/`PAS` (enumerations), `PRF`/`PRV`
//!   (valid ranges) and `CCA`/`CCS` (splines), and their verifiers as
//!   `CVS`/`CVE`/`CVP`.
//!
//! XTCE says much more than a MIB can hold; what's read is what the MIB
//! has room for. Which comparison is the APID, the service, the subtype
//! and the identification fields is up to `XtceOptions`: parameters it
//! names, or -- by default -- those whose names say so, the common way
//! XTCE files for PUS missions are written and the way Fenix's own
//! export writes them.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use roxmltree::Node;

use crate::parse::Problem;
use crate::row::{Row, RowSource};
use crate::schema;

/// Which parameters carry a packet's identification in an XTCE file's
/// restriction criteria, and which arguments carry a telecommand's. A
/// list that's empty falls back on names: APID; SERVICE or a TYPE that
/// isn't the packet type; SUBTYPE or SUBSERVICE; SID, a structure,
/// report or event id for the first identification field. Names compare
/// without case and without their space system path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct XtceOptions {
    pub apid: Vec<String>,
    pub service: Vec<String>,
    pub subtype: Vec<String>,
    pub pi1: Vec<String>,
    pub pi2: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Apid,
    Service,
    Subtype,
    Pi1,
    Pi2,
}

impl XtceOptions {
    /// What a parameter or argument called `name` identifies, if anything.
    fn role(&self, name: &str) -> Option<Role> {
        let name = base(name);
        let lists = [(&self.apid, Role::Apid), (&self.service, Role::Service), (&self.subtype, Role::Subtype), (&self.pi1, Role::Pi1), (&self.pi2, Role::Pi2)];
        for (list, role) in lists {
            if list.iter().any(|n| base(n).eq_ignore_ascii_case(name)) {
                return Some(role);
            }
        }
        let n = name.to_ascii_uppercase();
        let guess = if n.contains("APID") {
            Some(Role::Apid)
        } else if n.contains("SUBTYPE") || n.contains("SUB_TYPE") || n.contains("SUBSERVICE") || n.contains("SUB_SERVICE") {
            Some(Role::Subtype)
        } else if n.contains("SERVICE") || (n.ends_with("TYPE") && !n.contains("PACKET") && !n.contains("PKT") && !n.starts_with("CCSDS")) {
            Some(Role::Service)
        } else if n == "SID" || n.ends_with("_SID") || n.contains("STRUCTURE_ID") || n.contains("REPORT_ID") || n.contains("EVENT_ID") || n == "EVID" {
            Some(Role::Pi1)
        } else {
            None
        };
        // A role that was given names only takes those.
        match guess {
            Some(Role::Apid) if !self.apid.is_empty() => None,
            Some(Role::Service) if !self.service.is_empty() => None,
            Some(Role::Subtype) if !self.subtype.is_empty() => None,
            Some(Role::Pi1) if !self.pi1.is_empty() => None,
            other => other,
        }
    }
}

/// A reference without its space system path: `/SC/TM/APID` and
/// `../APID` are both `APID`.
fn base(r: &str) -> &str {
    r.rsplit('/').next().unwrap_or(r)
}

struct Builder<'a> {
    root: usize,
    label: &'a str,
    file: &'a Path,
    tables: HashMap<String, Vec<Row>>,
}

impl Builder<'_> {
    fn push(&mut self, table: &str, values: &[(&str, String)], line: usize) {
        let Some(columns) = schema::columns(table) else { return };
        let fields = columns
            .iter()
            .map(|c| (c.to_string(), values.iter().find(|(k, _)| k == c).map(|(_, v)| v.clone()).unwrap_or_default()))
            .collect();
        let source = RowSource { root_index: self.root, root_label: self.label.to_string(), table: table.to_string(), file: self.file.to_path_buf(), line };
        self.tables.entry(table.to_string()).or_default().push(Row { table: table.to_string(), fields, source });
    }
}

/// A parameter or argument type, as the MIB describes one.
#[derive(Clone, Default)]
struct Ty {
    kind: &'static str,
    ptc: u32,
    pfc: u32,
    bits: usize,
    /// Whether the type itself says how wide it is (else its base does).
    sized: bool,
    unit: String,
    little_endian: bool,
    /// Status texts: (raw from, raw to, text).
    enums: Vec<(i64, i64, String)>,
    points: Vec<(String, String)>,
    poly: Vec<String>,
    /// A calibration a MIB can't hold, by its XTCE name.
    other_cal: Option<&'static str>,
    /// The valid range of a value: (min, max).
    range: Option<(String, String)>,
    /// Static alarm limits: (`S` soft or `H` hard, low, high).
    alarms: Vec<(&'static str, String, String)>,
    /// An array's element type, and how many elements when fixed.
    element: Option<String>,
    count: usize,
    /// An aggregate's members: (name, type).
    members: Vec<(String, String)>,
}

fn pfc_for_int(bits: usize) -> u32 {
    match bits {
        4..=16 => bits as u32 - 4,
        24 => 13,
        32 => 14,
        48 => 15,
        64 => 16,
        _ => 12,
    }
}

/// A CUC time's PFC for `bits` (coarse octets up to 4, fine up to 3),
/// CDS with microseconds for 64; 0 (explicit) otherwise.
fn pfc_for_time(bits: usize, absolute: bool) -> u32 {
    match bits {
        64 if absolute => 2,
        8..=56 if bits.is_multiple_of(8) => {
            let octets = bits / 8;
            let coarse = octets.clamp(1, 4);
            let fine = octets - coarse;
            if fine > 3 {
                0
            } else {
                3 + (coarse as u32 - 1) * 4 + fine as u32
            }
        }
        _ => 0,
    }
}

/// What kind of type a node defines -- tag names without their
/// namespace: XTCE files use `xtce:`, another prefix, or none.
fn kind_of(n: Node) -> &'static str {
    match n.tag_name().name() {
        "IntegerParameterType" | "IntegerArgumentType" => "int",
        "FloatParameterType" | "FloatArgumentType" => "float",
        "EnumeratedParameterType" | "EnumeratedArgumentType" => "enum",
        "BooleanParameterType" | "BooleanArgumentType" => "bool",
        "StringParameterType" | "StringArgumentType" => "string",
        "BinaryParameterType" | "BinaryArgumentType" => "binary",
        "AbsoluteTimeParameterType" | "AbsoluteTimeArgumentType" => "abstime",
        "RelativeTimeParameterType" | "RelativeTimeArgumentType" => "reltime",
        "ArrayParameterType" | "ArrayArgumentType" => "array",
        "AggregateParameterType" | "AggregateArgumentType" => "aggregate",
        _ => "",
    }
}

fn is_arg_type(n: Node) -> bool {
    n.tag_name().name().ends_with("ArgumentType")
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn descendants<'a, 'i>(n: Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    n.descendants().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn fixed_value(n: Node) -> Option<i64> {
    descendants(n, "FixedValue").next()?.text()?.trim().parse().ok()
}

/// The raw width: the data encoding's, else the type's own attribute.
fn size_of(n: Node) -> Option<usize> {
    let encoding = n.descendants().find(|d| d.is_element() && d.tag_name().name().ends_with("DataEncoding"));
    if let Some(e) = encoding {
        if let Some(bits) = e.attribute("sizeInBits").and_then(|s| s.parse().ok()) {
            return Some(bits);
        }
        if matches!(e.tag_name().name(), "BinaryDataEncoding" | "StringDataEncoding") {
            if let Some(bits) = child(e, "SizeInBits").and_then(fixed_value) {
                return Some(bits as usize);
            }
        }
    }
    n.attribute("sizeInBits").and_then(|s| s.parse().ok())
}

/// The calibrator a value is calibrated with: the default one, else the
/// first context one.
fn calibrator<'a, 'i>(n: Node<'a, 'i>) -> Option<Node<'a, 'i>> {
    let holder = descendants(n, "DefaultCalibrator").next().or_else(|| descendants(n, "ContextCalibrator").next().and_then(|c| child(c, "Calibrator")))?;
    holder.children().find(Node::is_element)
}

fn read_type(n: Node) -> Ty {
    let kind = kind_of(n);
    let size = size_of(n);
    let mut t = Ty {
        kind,
        bits: size.unwrap_or(8),
        sized: size.is_some(),
        unit: descendants(n, "Unit").next().and_then(|u| u.text()).unwrap_or("").trim().to_string(),
        little_endian: n.descendants().any(|d| d.attribute("byteOrder") == Some("leastSignificantByteFirst")),
        ..Default::default()
    };
    match kind {
        "bool" => (t.ptc, t.pfc, t.bits, t.sized) = (1, 0, 1, true),
        "string" => (t.ptc, t.pfc) = (8, if t.sized { (t.bits / 8) as u32 } else { 0 }),
        "binary" => (t.ptc, t.pfc) = (7, if t.sized { (t.bits / 8) as u32 } else { 0 }),
        "abstime" => (t.ptc, t.pfc) = (9, pfc_for_time(t.bits, true)),
        "reltime" => (t.ptc, t.pfc) = (10, pfc_for_time(t.bits, false)),
        "float" if descendants(n, "FloatDataEncoding").next().is_some() => (t.ptc, t.pfc) = (5, if t.bits > 32 { 2 } else { 1 }),
        "enum" => {
            (t.ptc, t.pfc) = (3, pfc_for_int(t.bits));
            for e in descendants(n, "Enumeration") {
                let (Some(v), Some(label)) = (e.attribute("value").and_then(|v| v.parse().ok()), e.attribute("label")) else { continue };
                let hi = e.attribute("maxValue").and_then(|v| v.parse().ok()).unwrap_or(v);
                t.enums.push((v, hi, label.to_string()));
            }
        }
        "array" => {
            t.element = n.attribute("arrayTypeRef").map(|r| base(r).to_string());
            let dims: Vec<usize> = descendants(n, "Dimension")
                .filter_map(|d| {
                    let start = child(d, "StartingIndex").and_then(fixed_value)?;
                    let end = child(d, "EndingIndex").and_then(fixed_value)?;
                    usize::try_from(end - start + 1).ok()
                })
                .collect();
            t.count = if dims.is_empty() { 0 } else { dims.iter().product() };
        }
        "aggregate" => {
            for m in descendants(n, "Member") {
                if let (Some(name), Some(tref)) = (m.attribute("name"), m.attribute("typeRef")) {
                    t.members.push((name.to_string(), base(tref).to_string()));
                }
            }
        }
        _ => {
            let signed = n.attribute("signed") == Some("true")
                || descendants(n, "IntegerDataEncoding").any(|e| e.attribute("encoding").is_some_and(|x| x.contains("twos") || x == "signMagnitude" || x.contains("onesComplement")));
            (t.ptc, t.pfc) = (if signed { 4 } else { 3 }, pfc_for_int(t.bits));
        }
    }
    if let Some(cal) = calibrator(n) {
        match cal.tag_name().name() {
            "SplineCalibrator" => {
                for p in children(cal, "SplinePoint") {
                    if let (Some(r), Some(c)) = (p.attribute("raw"), p.attribute("calibrated")) {
                        t.points.push((r.to_string(), c.to_string()));
                    }
                }
            }
            "PolynomialCalibrator" => {
                let mut terms: Vec<(usize, String)> = children(cal, "Term").filter_map(|x| Some((x.attribute("exponent")?.parse().ok()?, x.attribute("coefficient")?.to_string()))).collect();
                terms.sort();
                if terms.last().is_some_and(|t| t.0 > 4) {
                    t.other_cal = Some("a polynomial of degree over 4");
                } else if !terms.is_empty() {
                    let top = terms.last().map(|t| t.0).unwrap_or(0);
                    t.poly = (0..=top).map(|i| terms.iter().find(|x| x.0 == i).map(|x| x.1.clone()).unwrap_or_else(|| "0".into())).collect();
                }
            }
            "MathOperationCalibrator" => t.other_cal = Some("MathOperationCalibrator"),
            _ => {}
        }
    }
    if let Some(r) = descendants(n, "ValidRange").next() {
        let lo = r.attribute("minInclusive").or_else(|| r.attribute("minExclusive"));
        let hi = r.attribute("maxInclusive").or_else(|| r.attribute("maxExclusive"));
        if lo.is_some() || hi.is_some() {
            t.range = Some((lo.unwrap_or("").to_string(), hi.unwrap_or("").to_string()));
        }
    }
    if let Some(ranges) = descendants(n, "DefaultAlarm").next().and_then(|a| descendants(a, "StaticAlarmRanges").next()) {
        for r in ranges.children().filter(Node::is_element) {
            let level = match r.tag_name().name() {
                "WatchRange" | "WarningRange" => "S",
                "DistressRange" | "CriticalRange" | "SevereRange" => "H",
                _ => continue,
            };
            let lo = r.attribute("minInclusive").or_else(|| r.attribute("minExclusive")).unwrap_or("");
            let hi = r.attribute("maxInclusive").or_else(|| r.attribute("maxExclusive")).unwrap_or("");
            t.alarms.push((level, lo.to_string(), hi.to_string()));
        }
    }
    t
}

/// `child` over `base`: what the child type doesn't say, its base does.
fn inherit(mut t: Ty, b: &Ty) -> Ty {
    // Its encoding, and so its codes, come from its base.
    if !t.sized {
        (t.ptc, t.pfc, t.bits, t.sized) = (b.ptc, b.pfc, b.bits, b.sized);
    }
    if t.unit.is_empty() {
        t.unit.clone_from(&b.unit);
    }
    t.little_endian |= b.little_endian;
    macro_rules! fill {
        ($($f:ident),*) => { $( if t.$f.is_empty() { t.$f = b.$f.clone(); } )* };
    }
    fill!(enums, points, poly, alarms, members);
    t.other_cal = t.other_cal.or(b.other_cal);
    t.range = t.range.or_else(|| b.range.clone());
    t.element = t.element.or_else(|| b.element.clone());
    if t.count == 0 {
        t.count = b.count;
    }
    t
}

/// Every type in the file by name, telemetry and command apart -- a
/// command's argument may name either -- with `baseType` followed.
struct Types<'a, 'i> {
    nodes: HashMap<(bool, String), Node<'a, 'i>>,
    done: HashMap<(bool, String), Ty>,
}

impl Types<'_, '_> {
    fn get(&mut self, arg: bool, name: &str) -> Option<Ty> {
        let name = base(name);
        let key = if self.nodes.contains_key(&(arg, name.to_string())) { (arg, name.to_string()) } else { (!arg, name.to_string()) };
        self.resolve(key, 0)
    }

    fn resolve(&mut self, key: (bool, String), depth: usize) -> Option<Ty> {
        if let Some(t) = self.done.get(&key) {
            return Some(t.clone());
        }
        let node = *self.nodes.get(&key)?;
        let mut t = read_type(node);
        if let Some(b) = node.attribute("baseType").filter(|_| depth < 16) {
            let b = base(b).to_string();
            let bkey = if self.nodes.contains_key(&(key.0, b.clone())) { (key.0, b) } else { (!key.0, b) };
            if let Some(bt) = self.resolve(bkey, depth + 1) {
                t = inherit(t, &bt);
            }
        }
        self.done.insert(key, t.clone());
        Some(t)
    }
}

fn describe(n: Node) -> String {
    child(n, "LongDescription").and_then(|d| d.text()).map(str::trim).filter(|d| !d.is_empty()).map(str::to_string).or_else(|| n.attribute("shortDescription").map(str::to_string)).unwrap_or_default()
}

/// The container a container extends.
fn base_of<'a>(n: Node<'a, 'a>) -> Option<&'a str> {
    child(n, "BaseContainer").and_then(|b| b.attribute("containerRef")).map(base)
}

/// Where an entry sits: from the packet's start, or after the previous
/// entry ends, or -- with no location -- right after it.
fn location(entry: Node, at: usize) -> usize {
    let Some(loc) = child(entry, "LocationInContainerInBits") else { return at };
    let Some(v) = fixed_value(loc) else { return at };
    match loc.attribute("referenceLocation").unwrap_or("previousEntry") {
        "containerStart" => v.max(0) as usize,
        _ => (at as i64 + v).max(0) as usize,
    }
}

/// How many times an entry repeats, when it says.
fn repeat(entry: Node) -> usize {
    child(entry, "RepeatEntry").and_then(|r| child(r, "Count")).and_then(fixed_value).map(|n| n.max(1) as usize).unwrap_or(1)
}

/// The comparisons a container's or command's restriction makes, on
/// equality: (parameter, value).
fn comparisons(n: Node) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for c in descendants(n, "Comparison") {
        let (Some(p), Some(v)) = (c.attribute("parameterRef"), c.attribute("value")) else { continue };
        if c.attribute("comparisonOperator").unwrap_or("==") == "==" {
            out.push((base(p).to_string(), v.trim().to_string()));
        }
    }
    for c in descendants(n, "Condition") {
        let Some(p) = child(c, "ParameterInstanceRef").and_then(|p| p.attribute("parameterRef")) else { continue };
        let op = child(c, "ComparisonOperator").and_then(|o| o.text()).unwrap_or("==").trim();
        let Some(v) = child(c, "Value").and_then(|v| v.text()) else { continue };
        if op == "==" {
            out.push((base(p).to_string(), v.trim().to_string()));
        }
    }
    out
}

/// Seconds in an `xs:duration` (`PT2.5S`, `PT1M`), or a plain number.
fn seconds(text: &str) -> Option<f64> {
    let t = text.trim();
    if let Ok(n) = t.parse() {
        return Some(n);
    }
    let rest = t.strip_prefix('P')?;
    let (days, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = 0.0;
    let mut number = String::new();
    for (part, units) in [(days, &[('D', 86400.0)][..]), (time, &[('H', 3600.0), ('M', 60.0), ('S', 1.0)][..])] {
        for c in part.chars() {
            match units.iter().find(|(u, _)| *u == c) {
                Some((_, scale)) => {
                    total += number.parse::<f64>().ok()? * scale;
                    number.clear();
                }
                None => number.push(c),
            }
        }
    }
    number.is_empty().then_some(total)
}

/// One parameter's place in a packet.
struct Slot {
    name: String,
    bit: usize,
    count: usize,
    /// Bits from one occurrence's start to the next's.
    step: usize,
}

/// The XTCE version a namespace is.
fn version_of(namespace: Option<&str>) -> Option<&'static str> {
    match namespace? {
        "http://www.omg.org/spec/XTCE/20180204" => Some("1.2"),
        "http://www.omg.org/space/xtce" => Some("1.1"),
        _ => None,
    }
}

/// The rows an XTCE file makes, by table, what couldn't be read, and the
/// XTCE version its namespace says.
pub fn read(root: usize, label: &str, file: &Path, options: &XtceOptions) -> (HashMap<String, Vec<Row>>, Vec<Problem>, Option<&'static str>) {
    let problem = |line: Option<usize>, message: String| Problem { root, file: file.to_path_buf(), line, message };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => return (HashMap::new(), vec![problem(None, format!("couldn't be read ({e})"))], None),
    };
    let doc = match roxmltree::Document::parse(&text) {
        Ok(d) => d,
        Err(e) => return (HashMap::new(), vec![problem(None, format!("isn't XML: {e}"))], None),
    };
    let top = doc.root_element();
    if top.tag_name().name() != "SpaceSystem" {
        return (HashMap::new(), vec![problem(None, format!("isn't XTCE: it starts with <{}>, not <SpaceSystem>", top.tag_name().name()))], None);
    }
    let version = version_of(top.tag_name().namespace());
    let line_of = |n: Node| doc.text_pos_at(n.range().start).row as usize;
    let mut b = Builder { root, label, file, tables: HashMap::new() };
    let mut problems = Vec::new();

    let mut types = Types { nodes: HashMap::new(), done: HashMap::new() };
    for n in doc.descendants().filter(|n| n.is_element() && !kind_of(*n).is_empty()) {
        if let Some(name) = n.attribute("name") {
            types.nodes.entry((is_arg_type(n), name.to_string())).or_insert(n);
        }
    }
    let mut cal_id = 0;
    let mut next_cal = || {
        cal_id += 1;
        cal_id
    };

    // TM parameters -- an aggregate's members each their own.
    let mut layouts: HashMap<String, Vec<(String, usize, Ty)>> = HashMap::new();
    let mut warned: HashSet<String> = HashSet::new();
    for n in descendants(top, "Parameter") {
        let (Some(name), Some(tref)) = (n.attribute("name"), n.attribute("parameterTypeRef")) else { continue };
        let t = types.get(false, tref).unwrap_or_default();
        let natur = match child(n, "ParameterProperties").and_then(|p| p.attribute("dataSource")) {
            Some("derived") => "D",
            Some("constant") => "C",
            _ => "R",
        };
        // (PCF name, bit offset within the parameter, type).
        let mut parts: Vec<(String, usize, Ty)> = Vec::new();
        let mut stack = vec![(name.to_string(), 0usize, t, 0usize)];
        while let Some((pname, off, t, depth)) = stack.pop() {
            if t.kind == "aggregate" && depth < 8 {
                let mut at = off;
                let mut members = Vec::new();
                for (m, mref) in &t.members {
                    let mt = types.get(false, mref).unwrap_or_default();
                    let bits = mt.bits;
                    members.push((format!("{pname}.{m}"), at, mt, depth + 1));
                    at += bits;
                }
                stack.extend(members.into_iter().rev());
            } else if t.kind == "array" {
                let et = t.element.as_deref().and_then(|e| types.get(false, e)).unwrap_or_default();
                let mut et = et;
                et.count = t.count.max(1);
                parts.push((pname, off, et));
            } else {
                parts.push((pname, off, t));
            }
        }
        for (pname, _, t) in &parts {
            if let Some(what) = t.other_cal {
                if warned.insert(pname.clone()) {
                    problems.push(problem(Some(line_of(n)), format!("parameter {pname}: calibrated with {what}, which a MIB can't hold -- read without it")));
                }
            }
            let mut categ = "N";
            let mut curtx = String::new();
            if !t.enums.is_empty() {
                categ = "S";
                curtx = format!("TXF{:05}", next_cal());
                b.push("txf", &[("TXF_NUMBR", curtx.clone()), ("TXF_DESCR", pname.clone()), ("TXF_RAWFMT", "U".into()), ("TXF_NALIAS", t.enums.len().to_string())], line_of(n));
                for (lo, hi, text) in &t.enums {
                    b.push("txp", &[("TXP_NUMBR", curtx.clone()), ("TXP_FROM", lo.to_string()), ("TXP_TO", hi.to_string()), ("TXP_ALTXT", text.clone())], line_of(n));
                }
            } else if !t.points.is_empty() {
                curtx = format!("CAF{:05}", next_cal());
                b.push("caf", &[("CAF_NUMBR", curtx.clone()), ("CAF_DESCR", pname.clone()), ("CAF_ENGFMT", "R".into()), ("CAF_RAWFMT", "U".into()), ("CAF_UNIT", t.unit.clone()), ("CAF_NCURVE", t.points.len().to_string())], line_of(n));
                for (r, c) in &t.points {
                    b.push("cap", &[("CAP_NUMBR", curtx.clone()), ("CAP_XVALS", r.clone()), ("CAP_YVALS", c.clone())], line_of(n));
                }
            } else if !t.poly.is_empty() {
                curtx = format!("MCF{:05}", next_cal());
                let mut v = vec![("MCF_IDENT", curtx.clone()), ("MCF_DESCR", pname.clone())];
                for (i, c) in t.poly.iter().enumerate() {
                    v.push((["MCF_POL1", "MCF_POL2", "MCF_POL3", "MCF_POL4", "MCF_POL5"][i], c.clone()));
                }
                b.push("mcf", &v, line_of(n));
            }
            if !t.alarms.is_empty() {
                let real = t.ptc == 5 || !t.points.is_empty() || !t.poly.is_empty();
                b.push(
                    "ocf",
                    &[
                        ("OCF_NAME", pname.clone()),
                        ("OCF_NBCHCK", t.alarms.len().to_string()),
                        ("OCF_NBOOL", t.alarms.len().to_string()),
                        ("OCF_INTER", if curtx.is_empty() { "U".into() } else { "C".into() }),
                        ("OCF_CODIN", if real { "R".into() } else { "I".into() }),
                    ],
                    line_of(n),
                );
                for (i, (level, lo, hi)) in t.alarms.iter().enumerate() {
                    b.push("ocp", &[("OCP_NAME", pname.clone()), ("OCP_POS", (i + 1).to_string()), ("OCP_TYPE", level.to_string()), ("OCP_LVALU", lo.clone()), ("OCP_HVALU", hi.clone())], line_of(n));
                }
            }
            b.push(
                "pcf",
                &[
                    ("PCF_NAME", pname.clone()),
                    ("PCF_DESCR", describe(n)),
                    ("PCF_UNIT", t.unit.clone()),
                    ("PCF_PTC", t.ptc.to_string()),
                    ("PCF_PFC", t.pfc.to_string()),
                    ("PCF_CATEG", categ.into()),
                    ("PCF_NATUR", natur.into()),
                    ("PCF_CURTX", curtx),
                    ("PCF_ENDIAN", if t.little_endian { "L".into() } else { String::new() }),
                ],
                line_of(n),
            );
        }
        layouts.insert(name.to_string(), parts);
    }

    // Packets: concrete containers no other one extends, identified by
    // the restrictions on them and on what they extend.
    let containers: HashMap<&str, Node> = descendants(top, "SequenceContainer").filter_map(|n| Some((n.attribute("name")?, n))).collect();
    let extended: HashSet<&str> = containers.values().filter_map(|n| base_of(*n)).collect();
    let included: HashSet<&str> = descendants(top, "ContainerRefEntry").filter_map(|e| e.attribute("containerRef")).map(base).collect();
    // Lays out `n`'s entries from bit `at`, into `out`; the end.
    fn lay_out(n: Node, mut at: usize, depth: usize, containers: &HashMap<&str, Node>, layouts: &HashMap<String, Vec<(String, usize, Ty)>>, out: &mut Vec<Slot>) -> usize {
        let Some(list) = child(n, "EntryList") else { return at };
        for e in list.children().filter(Node::is_element) {
            match e.tag_name().name() {
                "ParameterRefEntry" | "ArrayParameterRefEntry" => {
                    let Some(p) = e.attribute("parameterRef").map(base) else { continue };
                    let start = location(e, at);
                    let times = repeat(e);
                    let parts = layouts.get(p).map(Vec::as_slice).unwrap_or(&[]);
                    let width: usize = parts.iter().map(|(_, off, t)| off + t.bits * t.count.max(1)).max().unwrap_or(0);
                    for (name, off, t) in parts {
                        let count = t.count.max(1) * times;
                        let step = if parts.len() > 1 { width } else { t.bits };
                        out.push(Slot { name: name.clone(), bit: start + off, count, step });
                    }
                    if parts.is_empty() {
                        out.push(Slot { name: p.to_string(), bit: start, count: times, step: 0 });
                    }
                    at = start + width * times;
                }
                "ContainerRefEntry" if depth < 8 => {
                    let Some(c) = e.attribute("containerRef").map(base).and_then(|c| containers.get(c)) else { continue };
                    let start = location(e, at);
                    let mut end = start;
                    for _ in 0..repeat(e) {
                        end = lay_out(*c, end, depth + 1, containers, layouts, out);
                    }
                    at = end;
                }
                _ => {}
            }
        }
        at
    }
    let mut packets: Vec<(&str, Node)> = containers.iter().map(|(k, v)| (*k, *v)).collect();
    packets.sort_by_key(|(_, n)| n.range().start);
    let mut spid_counter = 1;
    let mut pics: HashSet<(String, String, String)> = HashSet::new();
    for (name, n) in packets {
        if n.attribute("abstract") == Some("true") || extended.contains(name) {
            continue;
        }
        // The chain: what it extends, outermost first.
        let mut chain = vec![n];
        while let Some(parent) = base_of(*chain.last().unwrap()).and_then(|p| containers.get(p)) {
            if chain.len() > 16 || chain.contains(parent) {
                break;
            }
            chain.push(*parent);
        }
        chain.reverse();
        let mut id: HashMap<&'static str, String> = HashMap::new();
        let mut pi_params: [Option<String>; 2] = [None, None];
        for c in &chain {
            let Some(criteria) = child(*c, "BaseContainer").and_then(|b| child(b, "RestrictionCriteria")) else { continue };
            for (p, v) in comparisons(criteria) {
                match options.role(&p) {
                    Some(Role::Apid) => id.insert("apid", v),
                    Some(Role::Service) => id.insert("service", v),
                    Some(Role::Subtype) => id.insert("subtype", v),
                    Some(Role::Pi1) => {
                        pi_params[0] = Some(p);
                        id.insert("pi1", v)
                    }
                    Some(Role::Pi2) => {
                        pi_params[1] = Some(p);
                        id.insert("pi2", v)
                    }
                    None => None,
                };
            }
        }
        let Some(apid) = id.get("apid").cloned() else {
            if !included.contains(name) {
                problems.push(problem(Some(line_of(n)), format!("container {name}: no APID comparison, so it isn't a packet Fenix can identify")));
            }
            continue;
        };
        let mut inherited = Vec::new();
        let mut at = 0;
        for c in &chain[..chain.len() - 1] {
            at = lay_out(*c, at, 0, &containers, &layouts, &mut inherited);
        }
        let mut own = Vec::new();
        lay_out(n, at, 0, &containers, &layouts, &mut own);
        let anc = |key: &str| descendants(n, "AncillaryData").find(|a| a.attribute("name").is_some_and(|x| x.eq_ignore_ascii_case(key))).and_then(|a| a.text()).map(|t| t.trim().to_string());
        let spid = n.attribute("shortDescription").and_then(|d| d.strip_prefix("SPID ")).map(|s| s.trim().to_string()).or_else(|| anc("SPID")).unwrap_or_else(|| {
            spid_counter += 1;
            format!("{}", 900_000 + spid_counter)
        });
        let (service, subtype) = (id.get("service").cloned().unwrap_or_else(|| "0".into()), id.get("subtype").cloned().unwrap_or_else(|| "0".into()));
        b.push(
            "pid",
            &[
                ("PID_TYPE", service.clone()),
                ("PID_STYPE", subtype.clone()),
                ("PID_APID", apid.clone()),
                ("PID_PI1_VAL", id.get("pi1").cloned().unwrap_or_else(|| "0".into())),
                ("PID_PI2_VAL", id.get("pi2").cloned().unwrap_or_else(|| "0".into())),
                ("PID_SPID", spid.clone()),
                ("PID_DESCR", describe(n)),
            ],
            line_of(n),
        );
        b.push("tpcf", &[("TPCF_SPID", spid.clone()), ("TPCF_NAME", name.to_string())], line_of(n));
        // Where the identification fields are, for a PIC.
        let place = |p: &Option<String>| -> Option<(usize, usize)> {
            let p = p.as_deref()?;
            let slot = inherited.iter().chain(own.iter()).find(|s| s.name == p)?;
            let bits = layouts.get(p)?.first()?.2.bits;
            (slot.bit % 8 == 0).then_some((slot.bit / 8, bits))
        };
        let (pi1, pi2) = (place(&pi_params[0]), place(&pi_params[1]));
        if pi1.is_some() && pics.insert((service.clone(), subtype.clone(), apid.clone())) {
            let (o1, w1) = pi1.map(|(o, w)| (o.to_string(), w.to_string())).unwrap_or_else(|| ("-1".into(), "0".into()));
            let (o2, w2) = pi2.map(|(o, w)| (o.to_string(), w.to_string())).unwrap_or_else(|| ("-1".into(), "0".into()));
            b.push("pic", &[("PIC_TYPE", service), ("PIC_STYPE", subtype), ("PIC_PI1_OFF", o1), ("PIC_PI1_WID", w1), ("PIC_PI2_OFF", o2), ("PIC_PI2_WID", w2), ("PIC_APID", apid)], line_of(n));
        }
        for s in own {
            let mut v = vec![("PLF_NAME", s.name), ("PLF_SPID", spid.clone()), ("PLF_OFFBY", (s.bit / 8).to_string()), ("PLF_OFFBI", (s.bit % 8).to_string())];
            if s.count > 1 {
                v.push(("PLF_NBOCC", s.count.to_string()));
                v.push(("PLF_LGOCC", s.step.to_string()));
            }
            b.push("plf", &v, line_of(n));
        }
    }

    // Telecommands, with what their bases give them.
    let commands: HashMap<&str, Node> = descendants(top, "MetaCommand").filter_map(|n| Some((n.attribute("name")?, n))).collect();
    let spids: HashMap<String, String> = b.tables.get("tpcf").into_iter().flatten().map(|r| (r.clean("TPCF_NAME").to_string(), r.clean("TPCF_SPID").to_string())).collect();
    let mut cpc_done = HashSet::new();
    let mut cvs_id = 0;
    let mut ordered: Vec<(&str, Node)> = commands.iter().map(|(k, v)| (*k, *v)).collect();
    ordered.sort_by_key(|(_, n)| n.range().start);
    for (name, n) in ordered {
        if n.attribute("abstract") == Some("true") {
            continue;
        }
        let mut chain = vec![n];
        while let Some(parent) = chain.last().and_then(|c| child(*c, "BaseMetaCommand")).and_then(|b| b.attribute("metaCommandRef")).and_then(|r| commands.get(base(r))) {
            if chain.len() > 16 || chain.contains(parent) {
                break;
            }
            chain.push(*parent);
        }
        // Nearest first: the command's own say wins.
        let args: Vec<(String, String, Option<String>, String)> = chain
            .iter()
            .flat_map(|c| child(*c, "ArgumentList").into_iter().flat_map(|l| children(l, "Argument")))
            .filter_map(|a| Some((a.attribute("name")?.to_string(), base(a.attribute("argumentTypeRef")?).to_string(), a.attribute("initialValue").map(str::to_string), describe(a))))
            .collect();
        let assigned: Vec<(String, String)> = chain
            .iter()
            .flat_map(|c| child(*c, "BaseMetaCommand").into_iter().flat_map(|b| descendants(b, "ArgumentAssignment")))
            .filter_map(|a| Some((a.attribute("argumentName")?.to_string(), a.attribute("argumentValue")?.to_string())))
            .collect();
        let ancillary = |key: &str| -> Option<String> {
            chain.iter().find_map(|c| child(*c, "AncillaryDataSet").and_then(|s| children(s, "AncillaryData").find(|a| a.attribute("name").is_some_and(|x| x.eq_ignore_ascii_case(key)))).and_then(|a| a.text()).map(|t| t.trim().to_string()))
        };
        let by_role = |role: Role| assigned.iter().find(|(a, _)| options.role(a) == Some(role)).map(|(_, v)| v.clone());
        let mut elements = 0;
        if let Some(cc) = child(n, "CommandContainer") {
            let mut at = 0;
            for e in child(cc, "EntryList").into_iter().flat_map(|l| l.children().filter(Node::is_element)) {
                match e.tag_name().name() {
                    "ArgumentRefEntry" | "ArrayArgumentRefEntry" => {
                        let Some(arg) = e.attribute("argumentRef").map(base) else { continue };
                        let Some((_, tref, initial, descr)) = args.iter().find(|(a, ..)| a == arg) else { continue };
                        let t = types.get(true, tref).unwrap_or_default();
                        let bit = location(e, at);
                        at = bit + t.bits;
                        let fixed = assigned.iter().find(|(a, _)| a == arg).map(|(_, v)| v.clone());
                        b.push(
                            "cdf",
                            &[
                                ("CDF_CNAME", name.to_string()),
                                ("CDF_ELTYPE", if fixed.is_some() { "F".into() } else { "E".into() }),
                                ("CDF_DESCR", descr.clone()),
                                ("CDF_ELLEN", t.bits.to_string()),
                                ("CDF_BIT", bit.to_string()),
                                ("CDF_GRPSIZE", "0".into()),
                                ("CDF_PNAME", arg.to_string()),
                                ("CDF_VALUE", fixed.unwrap_or_default()),
                            ],
                            line_of(e),
                        );
                        elements += 1;
                        if cpc_done.insert(arg.to_string()) {
                            let mut refs: Vec<(&str, String)> = Vec::new();
                            if !t.enums.is_empty() {
                                let id = format!("PAF{:05}", next_cal());
                                b.push("paf", &[("PAF_NUMBR", id.clone()), ("PAF_DESCR", arg.to_string()), ("PAF_RAWFMT", "U".into()), ("PAF_NALIAS", t.enums.len().to_string())], line_of(e));
                                for (lo, _, text) in &t.enums {
                                    b.push("pas", &[("PAS_NUMBR", id.clone()), ("PAS_ALTXT", text.clone()), ("PAS_ALVAL", lo.to_string())], line_of(e));
                                }
                                refs.push(("CPC_PAFREF", id));
                            } else if !t.points.is_empty() {
                                let id = format!("CCA{:05}", next_cal());
                                b.push("cca", &[("CCA_NUMBR", id.clone()), ("CCA_DESCR", arg.to_string()), ("CCA_ENGFMT", "R".into()), ("CCA_RAWFMT", "U".into()), ("CCA_UNIT", t.unit.clone()), ("CCA_NCURVE", t.points.len().to_string())], line_of(e));
                                // A TC curve goes engineering to raw.
                                for (r, c) in &t.points {
                                    b.push("ccs", &[("CCS_NUMBR", id.clone()), ("CCS_XVALS", c.clone()), ("CCS_YVALS", r.clone())], line_of(e));
                                }
                                refs.push(("CPC_CCAREF", id));
                            }
                            if let Some((lo, hi)) = &t.range {
                                let id = format!("PRF{:05}", next_cal());
                                let inter = if t.points.is_empty() { "R" } else { "E" };
                                b.push("prf", &[("PRF_NUMBR", id.clone()), ("PRF_DESCR", arg.to_string()), ("PRF_INTER", inter.into()), ("PRF_DSPFMT", if t.ptc == 5 { "R".into() } else { "I".into() }), ("PRF_NRANGE", "1".into()), ("PRF_UNIT", t.unit.clone())], line_of(e));
                                b.push("prv", &[("PRV_NUMBR", id.clone()), ("PRV_MINVAL", lo.clone()), ("PRV_MAXVAL", hi.clone())], line_of(e));
                                refs.push(("CPC_PRFREF", id));
                            }
                            let mut v = vec![
                                ("CPC_NAME", arg.to_string()),
                                ("CPC_DESCR", if descr.is_empty() { arg.to_string() } else { descr.clone() }),
                                ("CPC_PTC", t.ptc.to_string()),
                                ("CPC_PFC", t.pfc.to_string()),
                                ("CPC_UNIT", t.unit.clone()),
                                ("CPC_DEFVAL", initial.clone().unwrap_or_default()),
                                ("CPC_ENDIAN", if t.little_endian { "L".into() } else { String::new() }),
                            ];
                            v.extend(refs);
                            b.push("cpc", &v, line_of(e));
                        }
                    }
                    "FixedValueEntry" => {
                        let bits: usize = e.attribute("sizeInBits").and_then(|s| s.parse().ok()).unwrap_or(0);
                        let value = e.attribute("binaryValue").and_then(|v| i64::from_str_radix(v, 16).ok()).unwrap_or(0);
                        let bit = location(e, at);
                        at = bit + bits;
                        let ename = e.attribute("name").unwrap_or("");
                        let pname = if ename == "spare" { String::new() } else { ename.to_string() };
                        b.push("cdf", &[("CDF_CNAME", name.to_string()), ("CDF_ELTYPE", if pname.is_empty() { "A".into() } else { "F".into() }), ("CDF_ELLEN", bits.to_string()), ("CDF_BIT", bit.to_string()), ("CDF_GRPSIZE", "0".into()), ("CDF_PNAME", pname.clone()), ("CDF_VALUE", value.to_string())], line_of(e));
                        if !pname.is_empty() && cpc_done.insert(pname.clone()) {
                            b.push("cpc", &[("CPC_NAME", pname.clone()), ("CPC_DESCR", pname.clone()), ("CPC_PTC", "3".into()), ("CPC_PFC", pfc_for_int(bits).to_string())], line_of(e));
                        }
                        elements += 1;
                    }
                    _ => {}
                }
            }
        }
        let critical = chain.iter().find_map(|c| child(*c, "DefaultSignificance")).and_then(|s| s.attribute("consequenceLevel")).is_some_and(|l| matches!(l, "vital" | "critical" | "forbidden"));
        b.push(
            "ccf",
            &[
                ("CCF_CNAME", name.to_string()),
                ("CCF_DESCR", n.attribute("shortDescription").map(str::to_string).unwrap_or_else(|| describe(n))),
                ("CCF_DESCR2", child(n, "LongDescription").and_then(|d| d.text()).unwrap_or("").trim().to_string()),
                ("CCF_CRITICAL", if critical { "Y".into() } else { "N".into() }),
                ("CCF_TYPE", ancillary("TYPE").or_else(|| ancillary("SERVICE")).or_else(|| by_role(Role::Service)).unwrap_or_default()),
                ("CCF_STYPE", ancillary("SUBTYPE").or_else(|| by_role(Role::Subtype)).unwrap_or_default()),
                ("CCF_APID", ancillary("APID").or_else(|| by_role(Role::Apid)).unwrap_or_default()),
                ("CCF_NPARS", elements.to_string()),
                ("CCF_ACK", ancillary("ACK").unwrap_or_default()),
            ],
            line_of(n),
        );
        // Verification: each verifier a stage.
        let Some(verifiers) = chain.iter().find_map(|c| child(*c, "VerifierSet")) else { continue };
        for v in verifiers.children().filter(Node::is_element) {
            let stage = match v.tag_name().name() {
                "ReceivedVerifier" | "AcceptedVerifier" => "A",
                "QueuedVerifier" | "ExecutionVerifier" => "S",
                "CompleteVerifier" => "C",
                _ => continue,
            };
            cvs_id += 1;
            let window = descendants(v, "CheckWindow").next();
            let start = window.and_then(|w| w.attribute("timeToStartChecking")).and_then(seconds).unwrap_or(0.0);
            let stop = window.and_then(|w| w.attribute("timeToStopChecking")).and_then(seconds).unwrap_or(start);
            let report = child(v, "ContainerRef").and_then(|c| c.attribute("containerRef")).map(base);
            let checks = comparisons(v);
            let source = if report.is_some() && checks.is_empty() { "R" } else { "V" };
            b.push(
                "cvs",
                &[
                    ("CVS_ID", cvs_id.to_string()),
                    ("CVS_TYPE", stage.into()),
                    ("CVS_SOURCE", source.into()),
                    ("CVS_START", format!("{}", start.round() as i64)),
                    ("CVS_INTERVAL", format!("{}", (stop - start).max(0.0).round() as i64)),
                    ("CVS_SPID", report.and_then(|r| spids.get(r)).cloned().unwrap_or_default()),
                ],
                line_of(v),
            );
            for (p, value) in checks {
                b.push("cve", &[("CVE_CVSID", cvs_id.to_string()), ("CVE_PARNAM", p), ("CVE_INTER", "R".into()), ("CVE_VAL", value), ("CVE_CHECK", "B".into())], line_of(v));
            }
            b.push("cvp", &[("CVP_TASK", name.to_string()), ("CVP_TYPE", "C".into()), ("CVP_CVSID", cvs_id.to_string())], line_of(v));
        }
    }
    (b.tables, problems, version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::OffsetBase;
    use crate::set::tests::{fixture, temp_root};
    use crate::set::{CalKind, Kind, MibSet};
    use crate::{MibRoot, ReadOptions};

    #[test]
    fn a_mib_written_as_xtce_reads_back_as_the_same_definitions() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let xml = crate::generate::xtce(&set, &fenix_ccsds::Profile::default(), OffsetBase::AfterHeaders);
        let dir = temp_root("xtce");
        let file = dir.path.join("mission.xml");
        std::fs::write(&file, &xml).unwrap();
        let back = MibSet::load(vec![MibRoot { label: "XTCE".into(), path: file.clone() }], None);
        let tc = back.find(Kind::Telecommand, 0, "ZTC08101").unwrap_or_else(|| panic!("{:?} {:?}", back.problems(), back.entries(Kind::Telecommand).len()));
        let e = back.get(tc);
        assert_eq!((e.row.clean("CCF_TYPE"), e.row.clean("CCF_STYPE"), e.row.clean("CCF_APID")), ("8", "1", "1010"));
        let els = crate::detail::tc_elements(&back, tc);
        let mode = els.iter().find(|el| el.name == "PTH00102").unwrap();
        assert_eq!(mode.allowed, "OFF ON AUTO");
        assert_eq!((mode.bit, mode.bits), (Some(16), Some(8)));
        assert!(els.iter().any(|el| el.fixed.as_deref() == Some("12")), "the fixed function id");
        let temp = back.find(Kind::TmParam, 0, "NTH00123").unwrap();
        assert_eq!(back.get(temp).cols, vec!["uint16".to_string(), "degC".into(), "numeric".into()]);
        let pkt = back.find(Kind::TmPacket, 0, "30211").unwrap();
        assert_eq!(back.get(pkt).alias, "TCS_HK_FAST");
        let slots = crate::detail::packet_slots(&back, pkt);
        assert!(slots.iter().any(|s| s.name == "NTH00123"));
        assert_eq!(back.root_info(0).format.as_deref(), Some("XTCE 1.2"));
        std::fs::remove_dir_all(&root.path).ok();
        std::fs::remove_dir_all(&dir.path).ok();
    }

    /// An XTCE 1.1 file written the way other tools write them: types
    /// that extend others, a header container the packets extend, the
    /// identification split between them, an included container, a
    /// repeat, an array, an aggregate, alarms, a base command whose
    /// assignments give the PUS fields, valid ranges and verifiers.
    const OTHER_TOOL: &str = r#"<?xml version="1.0"?>
<SpaceSystem xmlns="http://www.omg.org/space/xtce" name="SC">
  <TelemetryMetaData>
    <ParameterTypeSet>
      <IntegerParameterType name="u8" signed="false"><IntegerDataEncoding sizeInBits="8"/></IntegerParameterType>
      <IntegerParameterType name="u11"><IntegerDataEncoding sizeInBits="11"/></IntegerParameterType>
      <IntegerParameterType name="u16"><UnitSet><Unit>V</Unit></UnitSet><IntegerDataEncoding sizeInBits="16" byteOrder="leastSignificantByteFirst"/></IntegerParameterType>
      <FloatParameterType name="temp_t">
        <UnitSet><Unit>degC</Unit></UnitSet>
        <IntegerDataEncoding sizeInBits="12">
          <DefaultCalibrator><PolynomialCalibrator><Term exponent="0" coefficient="-50"/><Term exponent="1" coefficient="0.1"/></PolynomialCalibrator></DefaultCalibrator>
        </IntegerDataEncoding>
        <DefaultAlarm><StaticAlarmRanges><WarningRange minInclusive="-10" maxInclusive="40"/><CriticalRange minInclusive="-20" maxInclusive="60"/></StaticAlarmRanges></DefaultAlarm>
      </FloatParameterType>
      <FloatParameterType name="hot_t" baseType="temp_t"/>
      <AbsoluteTimeParameterType name="obt_t"><Encoding><IntegerDataEncoding sizeInBits="48"/></Encoding></AbsoluteTimeParameterType>
      <ArrayParameterType name="cells_t" arrayTypeRef="u16"><DimensionList><Dimension><StartingIndex><FixedValue>0</FixedValue></StartingIndex><EndingIndex><FixedValue>3</FixedValue></EndingIndex></Dimension></DimensionList></ArrayParameterType>
      <AggregateParameterType name="vec_t"><MemberList><Member name="x" typeRef="u8"/><Member name="y" typeRef="u8"/></MemberList></AggregateParameterType>
    </ParameterTypeSet>
    <ParameterSet>
      <Parameter name="APID" parameterTypeRef="u11"/>
      <Parameter name="ServiceType" parameterTypeRef="u8"/>
      <Parameter name="ServiceSubType" parameterTypeRef="u8"/>
      <Parameter name="SID" parameterTypeRef="u8"/>
      <Parameter name="OBT" parameterTypeRef="obt_t"/>
      <Parameter name="BATT_V" parameterTypeRef="/SC/u16"><LongDescription>Battery voltage</LongDescription></Parameter>
      <Parameter name="TANK_T" parameterTypeRef="hot_t"/>
      <Parameter name="CELLS" parameterTypeRef="cells_t"/>
      <Parameter name="SUN" parameterTypeRef="vec_t"/>
    </ParameterSet>
    <ContainerSet>
      <SequenceContainer name="Header">
        <EntryList>
          <ParameterRefEntry parameterRef="APID"><LocationInContainerInBits referenceLocation="containerStart"><FixedValue>5</FixedValue></LocationInContainerInBits></ParameterRefEntry>
          <ParameterRefEntry parameterRef="ServiceType"><LocationInContainerInBits referenceLocation="containerStart"><FixedValue>56</FixedValue></LocationInContainerInBits></ParameterRefEntry>
          <ParameterRefEntry parameterRef="ServiceSubType"/>
          <ParameterRefEntry parameterRef="SID"><LocationInContainerInBits referenceLocation="containerStart"><FixedValue>128</FixedValue></LocationInContainerInBits></ParameterRefEntry>
        </EntryList>
      </SequenceContainer>
      <SequenceContainer name="HK">
        <EntryList/>
        <BaseContainer containerRef="Header"><RestrictionCriteria><ComparisonList><Comparison parameterRef="APID" value="100"/><Comparison parameterRef="ServiceType" value="3"/></ComparisonList></RestrictionCriteria></BaseContainer>
      </SequenceContainer>
      <SequenceContainer name="Power">
        <EntryList><ParameterRefEntry parameterRef="BATT_V"/><ParameterRefEntry parameterRef="CELLS"/></EntryList>
      </SequenceContainer>
      <SequenceContainer name="HK_POWER">
        <AncillaryDataSet><AncillaryData name="SPID">5001</AncillaryData></AncillaryDataSet>
        <EntryList>
          <ParameterRefEntry parameterRef="OBT"/>
          <ContainerRefEntry containerRef="Power"/>
          <ParameterRefEntry parameterRef="TANK_T"><RepeatEntry><Count><FixedValue>2</FixedValue></Count></RepeatEntry></ParameterRefEntry>
          <ParameterRefEntry parameterRef="SUN"/>
        </EntryList>
        <BaseContainer containerRef="HK"><RestrictionCriteria><BooleanExpression><ANDedConditions>
          <Condition><ParameterInstanceRef parameterRef="ServiceSubType"/><ComparisonOperator>==</ComparisonOperator><Value>25</Value></Condition>
          <Condition><ParameterInstanceRef parameterRef="SID"/><ComparisonOperator>==</ComparisonOperator><Value>7</Value></Condition>
        </ANDedConditions></BooleanExpression></RestrictionCriteria></BaseContainer>
      </SequenceContainer>
      <SequenceContainer name="TC_ACK"><EntryList/><BaseContainer containerRef="Header"><RestrictionCriteria><ComparisonList><Comparison parameterRef="APID" value="100"/><Comparison parameterRef="ServiceType" value="1"/><Comparison parameterRef="ServiceSubType" value="7"/></ComparisonList></RestrictionCriteria></BaseContainer></SequenceContainer>
    </ContainerSet>
  </TelemetryMetaData>
  <CommandMetaData>
    <ArgumentTypeSet>
      <IntegerArgumentType name="u8a"><IntegerDataEncoding sizeInBits="8"/></IntegerArgumentType>
      <IntegerArgumentType name="level_t"><IntegerDataEncoding sizeInBits="16"/><ValidRangeSet><ValidRange minInclusive="0" maxInclusive="1000"/></ValidRangeSet></IntegerArgumentType>
    </ArgumentTypeSet>
    <MetaCommandSet>
      <MetaCommand name="PUS_TC" abstract="true">
        <ArgumentList><Argument name="APID" argumentTypeRef="u8a"/><Argument name="Type" argumentTypeRef="u8a"/><Argument name="SubType" argumentTypeRef="u8a"/></ArgumentList>
      </MetaCommand>
      <MetaCommand name="SET_HEATER">
        <BaseMetaCommand metaCommandRef="PUS_TC"><ArgumentAssignmentList>
          <ArgumentAssignment argumentName="APID" argumentValue="100"/>
          <ArgumentAssignment argumentName="Type" argumentValue="8"/>
          <ArgumentAssignment argumentName="SubType" argumentValue="1"/>
        </ArgumentAssignmentList></BaseMetaCommand>
        <ArgumentList><Argument name="LEVEL" argumentTypeRef="level_t" initialValue="10"/></ArgumentList>
        <CommandContainer name="SET_HEATER_cc"><EntryList><ArgumentRefEntry argumentRef="LEVEL"/></EntryList></CommandContainer>
        <DefaultSignificance consequenceLevel="critical"/>
        <VerifierSet>
          <AcceptedVerifier><ContainerRef containerRef="TC_ACK"/><CheckWindow timeToStopChecking="PT5S"/></AcceptedVerifier>
          <CompleteVerifier><ComparisonList><Comparison parameterRef="TANK_T" value="20"/></ComparisonList><CheckWindow timeToStartChecking="PT2S" timeToStopChecking="PT1M"/></CompleteVerifier>
        </VerifierSet>
      </MetaCommand>
    </MetaCommandSet>
  </CommandMetaData>
</SpaceSystem>
"#;

    fn other_tool() -> (MibSet, MibRoot) {
        let dir = temp_root("xtce-other");
        let file = dir.path.join("sc.xtce");
        std::fs::write(&file, OTHER_TOOL).unwrap();
        (MibSet::load(vec![MibRoot { label: "SC".into(), path: file }], None), dir)
    }

    fn rows<'a>(set: &'a MibSet, table: &str) -> Vec<&'a Row> {
        set.index().rows(table).iter().collect()
    }

    #[test]
    fn packets_take_their_identification_from_what_they_extend() {
        let (set, dir) = other_tool();
        let pid: Vec<_> = rows(&set, "pid").into_iter().filter(|r| r.clean("PID_SPID") == "5001").collect();
        assert_eq!(pid.len(), 1, "{:?}", set.problems());
        assert_eq!((pid[0].clean("PID_APID"), pid[0].clean("PID_TYPE"), pid[0].clean("PID_STYPE"), pid[0].clean("PID_PI1_VAL")), ("100", "3", "25", "7"));
        // Header and HK are only extended: no packets of their own, and no
        // problem about them either.
        assert!(rows(&set, "tpcf").iter().all(|r| !matches!(r.clean("TPCF_NAME"), "Header" | "HK" | "Power")));
        assert!(set.problems().iter().all(|p| !p.message.contains("container")), "{:?}", set.problems());
        let pic = rows(&set, "pic");
        assert_eq!((pic[0].clean("PIC_PI1_OFF"), pic[0].clean("PIC_PI1_WID"), pic[0].clean("PIC_PI2_OFF")), ("16", "8", "-1"));
        assert_eq!(set.root_info(0).format.as_deref(), Some("XTCE 1.1"));
        std::fs::remove_dir_all(&dir.path).ok();
    }

    #[test]
    fn a_packet_is_laid_out_after_what_it_inherits_with_what_it_includes() {
        let (set, dir) = other_tool();
        let plf: Vec<(String, String, String, String)> = rows(&set, "plf")
            .iter()
            .filter(|r| r.clean("PLF_SPID") == "5001")
            .map(|r| (r.clean("PLF_NAME").to_string(), r.clean("PLF_OFFBY").to_string(), r.clean("PLF_NBOCC").to_string(), r.clean("PLF_LGOCC").to_string()))
            .collect();
        let at = |n: &str| plf.iter().find(|p| p.0 == n).unwrap_or_else(|| panic!("{n} in {plf:?}")).clone();
        // The header ends with SID at bit 128 + 8 = 136, octet 17.
        assert_eq!(at("OBT").1, "17");
        assert_eq!(at("BATT_V").1, "23");
        assert_eq!((at("CELLS").1, at("CELLS").2, at("CELLS").3), ("25".into(), "4".into(), "16".into()));
        assert_eq!((at("TANK_T").2, at("TANK_T").3), ("2".into(), "12".into()));
        assert!(plf.iter().any(|p| p.0 == "SUN.x") && plf.iter().any(|p| p.0 == "SUN.y"));
        // The header's own entries aren't the packet's parameters.
        assert!(plf.iter().all(|p| p.0 != "APID"));
        std::fs::remove_dir_all(&dir.path).ok();
    }

    #[test]
    fn types_follow_their_base_and_say_what_a_mib_can() {
        let (set, dir) = other_tool();
        let pcf = |n: &str| rows(&set, "pcf").into_iter().find(|r| r.clean("PCF_NAME") == n).unwrap_or_else(|| panic!("{n}"));
        let tank = pcf("TANK_T");
        assert_eq!((tank.clean("PCF_PTC"), tank.clean("PCF_PFC"), tank.clean("PCF_UNIT")), ("3", "8", "degC"));
        let mcf = rows(&set, "mcf").into_iter().find(|r| r.clean("MCF_IDENT") == tank.clean("PCF_CURTX")).unwrap();
        assert_eq!((mcf.clean("MCF_POL1"), mcf.clean("MCF_POL2")), ("-50", "0.1"));
        let ocp: Vec<_> = rows(&set, "ocp").into_iter().filter(|r| r.clean("OCP_NAME") == "TANK_T").map(|r| (r.clean("OCP_TYPE").to_string(), r.clean("OCP_LVALU").to_string(), r.clean("OCP_HVALU").to_string())).collect();
        assert_eq!(ocp, vec![("S".into(), "-10".into(), "40".into()), ("H".into(), "-20".into(), "60".into())]);
        assert_eq!((pcf("OBT").clean("PCF_PTC"), pcf("OBT").clean("PCF_PFC")), ("9", "17"));
        assert_eq!(pcf("BATT_V").clean("PCF_ENDIAN"), "L");
        assert_eq!(pcf("BATT_V").clean("PCF_DESCR"), "Battery voltage");
        assert_eq!(pcf("CELLS").clean("PCF_PFC"), "12");
        std::fs::remove_dir_all(&dir.path).ok();
    }

    #[test]
    fn commands_take_what_their_base_assigns_ranges_and_verifiers() {
        let (set, dir) = other_tool();
        assert!(set.find(Kind::Telecommand, 0, "PUS_TC").is_none(), "an abstract command isn't one");
        let tc = set.find(Kind::Telecommand, 0, "SET_HEATER").unwrap();
        let r = &set.get(tc).row;
        assert_eq!((r.clean("CCF_APID"), r.clean("CCF_TYPE"), r.clean("CCF_STYPE"), r.clean("CCF_CRITICAL")), ("100", "8", "1", "Y"));
        let level = set.find(Kind::TcParam, 0, "LEVEL").unwrap();
        let cpc = &set.get(level).row;
        assert_eq!(cpc.clean("CPC_DEFVAL"), "10");
        let range = set.find_cal(0, CalKind::Range, cpc.clean("CPC_PRFREF")).unwrap();
        assert!(set.get(range).cols.iter().any(|c| c.contains("1000")), "{:?}", set.get(range).cols);
        let cvs = rows(&set, "cvs");
        assert_eq!(cvs.len(), 2);
        assert_eq!((cvs[0].clean("CVS_TYPE"), cvs[0].clean("CVS_SOURCE"), cvs[0].clean("CVS_INTERVAL")), ("A", "R", "5"));
        assert_eq!((cvs[1].clean("CVS_TYPE"), cvs[1].clean("CVS_START"), cvs[1].clean("CVS_INTERVAL")), ("C", "2", "58"));
        assert!(!cvs[0].clean("CVS_SPID").is_empty());
        assert_eq!(rows(&set, "cve")[0].clean("CVE_PARNAM"), "TANK_T");
        assert_eq!(rows(&set, "cvp").len(), 2);
        std::fs::remove_dir_all(&dir.path).ok();
    }

    #[test]
    fn identification_parameters_can_be_named() {
        let dir = temp_root("xtce-named");
        let file = dir.path.join("sc.xml");
        std::fs::write(&file, OTHER_TOOL.replace("\"SID\"", "\"STRUCT\"")).unwrap();
        let root = MibRoot { label: "SC".into(), path: file };
        // Without being told, STRUCT says nothing.
        let set = MibSet::load(vec![root.clone()], None);
        let pid = set.index().rows("pid").iter().find(|r| r.clean("PID_SPID") == "5001").unwrap();
        assert_eq!(pid.clean("PID_PI1_VAL"), "0");
        let options = ReadOptions { xtce: XtceOptions { pi1: vec!["STRUCT".into()], ..Default::default() }, ..Default::default() };
        let set = MibSet::load_with(vec![root], None, options);
        let pid = set.index().rows("pid").iter().find(|r| r.clean("PID_SPID") == "5001").unwrap();
        assert_eq!(pid.clean("PID_PI1_VAL"), "7");
        std::fs::remove_dir_all(&dir.path).ok();
    }

    #[test]
    fn roles_by_name_and_by_setting() {
        let guess = XtceOptions::default();
        assert_eq!(guess.role("/SC/CCSDS_APID"), Some(Role::Apid));
        assert_eq!(guess.role("PUS_SUBTYPE"), Some(Role::Subtype));
        assert_eq!(guess.role("ServiceSubType"), Some(Role::Subtype));
        assert_eq!(guess.role("PUS_SERVICE"), Some(Role::Service));
        assert_eq!(guess.role("MessageType"), Some(Role::Service));
        assert_eq!(guess.role("CCSDS_PacketType"), None);
        assert_eq!(guess.role("PKT_TYPE"), None);
        assert_eq!(guess.role("HK_SID"), Some(Role::Pi1));
        let set = XtceOptions { apid: vec!["APPLICATION".into()], service: vec!["SVC".into()], ..Default::default() };
        assert_eq!(set.role("APPLICATION"), Some(Role::Apid));
        assert_eq!(set.role("CCSDS_APID"), None, "named roles take only their names");
        assert_eq!(set.role("svc"), Some(Role::Service));
        assert_eq!(set.role("PUS_SUBTYPE"), Some(Role::Subtype));
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(seconds("PT2.5S"), Some(2.5));
        assert_eq!(seconds("PT1M30S"), Some(90.0));
        assert_eq!(seconds("P1DT1H"), Some(90000.0));
        assert_eq!(seconds("12"), Some(12.0));
        assert_eq!(seconds("soon"), None);
        assert_eq!(pfc_for_time(32, true), 15);
        assert_eq!(pfc_for_time(48, true), 17);
        assert_eq!(pfc_for_time(64, true), 2);
        assert_eq!(pfc_for_time(64, false), 0);
    }

    #[test]
    fn a_file_that_isnt_xtce_says_so() {
        let dir = temp_root("xtce-not");
        let file = dir.path.join("pom.xml");
        std::fs::write(&file, "<project/>").unwrap();
        let set = MibSet::load(vec![MibRoot { label: "X".into(), path: file }], None);
        assert!(set.problems()[0].message.contains("isn't XTCE"));
        std::fs::remove_dir_all(&dir.path).ok();
    }
}
