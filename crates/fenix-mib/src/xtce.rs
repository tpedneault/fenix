//! An XTCE (CCSDS 660.0-B) file read as a MIB: its parameters, packet
//! containers and meta-commands turned into the rows of the MIB tables
//! they correspond to (`PCF`, `PID`/`TPCF`/`PLF`, `CCF`/`CDF`/`CPC`, and
//! calibrations as `CAF`/`CAP`, `TXF`/`TXP`, `PAF`/`PAS`, `MCF`), so the
//! MIB pages, the inspector and the insert form work the same on it.
//!
//! XTCE says much more than a MIB can hold; what's read is what the MIB
//! has room for. A packet's identification is taken from its container's
//! restriction comparisons on parameters whose names say APID, service
//! (or type) and subtype -- the common way XTCE files for PUS missions
//! are written, and the way Fenix's own export writes them.

use std::collections::HashMap;
use std::path::Path;

use crate::parse::Problem;
use crate::row::{Row, RowSource};
use crate::schema;

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
    ptc: u32,
    pfc: u32,
    bits: usize,
    unit: String,
    /// Status texts: (raw, text).
    enums: Vec<(i64, i64, String)>,
    points: Vec<(String, String)>,
    poly: Vec<String>,
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

fn local(n: roxmltree::Node) -> &'static str {
    // Tag names without their namespace: XTCE files use `xtce:` or none.
    match n.tag_name().name() {
        "IntegerParameterType" | "IntegerArgumentType" => "int",
        "FloatParameterType" | "FloatArgumentType" => "float",
        "EnumeratedParameterType" | "EnumeratedArgumentType" => "enum",
        "BooleanParameterType" | "BooleanArgumentType" => "bool",
        "StringParameterType" | "StringArgumentType" => "string",
        _ => "",
    }
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn descendants<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &'static str) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.descendants().filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn size_of(n: roxmltree::Node) -> Option<usize> {
    n.attribute("sizeInBits").and_then(|s| s.parse().ok()).or_else(|| n.descendants().find(|d| d.is_element() && d.attribute("sizeInBits").is_some()).and_then(|d| d.attribute("sizeInBits")?.parse().ok()))
}

fn read_type(n: roxmltree::Node) -> Ty {
    let mut t = Ty { unit: descendants(n, "Unit").next().and_then(|u| u.text()).unwrap_or("").trim().to_string(), ..Default::default() };
    let bits = size_of(n).unwrap_or(8);
    t.bits = bits;
    match local(n) {
        "bool" => (t.ptc, t.pfc, t.bits) = (1, 0, 1),
        "string" => {
            let bits = descendants(n, "FixedValue").next().and_then(|v| v.text()?.trim().parse().ok()).unwrap_or(bits);
            (t.ptc, t.pfc, t.bits) = (8, (bits / 8) as u32, bits);
        }
        "float" if descendants(n, "FloatDataEncoding").next().is_some() => (t.ptc, t.pfc) = (5, if bits > 32 { 2 } else { 1 }),
        "enum" => {
            (t.ptc, t.pfc) = (3, pfc_for_int(bits));
            for e in descendants(n, "Enumeration") {
                let (Some(v), Some(label)) = (e.attribute("value").and_then(|v| v.parse().ok()), e.attribute("label")) else { continue };
                let hi = e.attribute("maxValue").and_then(|v| v.parse().ok()).unwrap_or(v);
                t.enums.push((v, hi, label.to_string()));
            }
        }
        _ => {
            let signed = n.attribute("signed") == Some("true") || descendants(n, "IntegerDataEncoding").any(|e| e.attribute("encoding").is_some_and(|x| x.contains("twos") || x == "signMagnitude" || x.contains("onesComplement")));
            (t.ptc, t.pfc) = (if signed { 4 } else { 3 }, pfc_for_int(bits));
        }
    }
    for p in descendants(n, "SplinePoint") {
        if let (Some(r), Some(c)) = (p.attribute("raw"), p.attribute("calibrated")) {
            t.points.push((r.to_string(), c.to_string()));
        }
    }
    let mut terms: Vec<(usize, String)> = descendants(n, "Term").filter_map(|x| Some((x.attribute("exponent")?.parse().ok()?, x.attribute("coefficient")?.to_string()))).collect();
    terms.sort();
    if !terms.is_empty() {
        let top = terms.last().map(|t| t.0).unwrap_or(0).min(4);
        t.poly = (0..=top).map(|i| terms.iter().find(|x| x.0 == i).map(|x| x.1.clone()).unwrap_or_else(|| "0".into())).collect();
    }
    t
}

fn describe(n: roxmltree::Node) -> String {
    child(n, "LongDescription").and_then(|d| d.text()).map(str::trim).filter(|d| !d.is_empty()).map(str::to_string).or_else(|| n.attribute("shortDescription").map(str::to_string)).unwrap_or_default()
}

fn location(entry: roxmltree::Node) -> Option<usize> {
    let loc = child(entry, "LocationInContainerInBits")?;
    if loc.attribute("referenceLocation").is_some_and(|r| r != "containerStart") {
        return None;
    }
    descendants(loc, "FixedValue").next()?.text()?.trim().parse().ok()
}

/// The rows an XTCE file makes, by table, and what couldn't be read.
pub fn read(root: usize, label: &str, file: &Path) -> (HashMap<String, Vec<Row>>, Vec<Problem>) {
    let problem = |message: String| Problem { root, file: file.to_path_buf(), line: None, message };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => return (HashMap::new(), vec![problem(format!("couldn't be read ({e})"))]),
    };
    let doc = match roxmltree::Document::parse(&text) {
        Ok(d) => d,
        Err(e) => return (HashMap::new(), vec![problem(format!("isn't XML: {e}"))]),
    };
    let line_of = |n: roxmltree::Node| doc.text_pos_at(n.range().start).row as usize;
    let mut b = Builder { root, label, file, tables: HashMap::new() };
    let mut problems = Vec::new();

    // Types, by name.
    let mut types: HashMap<String, Ty> = HashMap::new();
    for n in doc.descendants().filter(|n| n.is_element() && !local(*n).is_empty()) {
        if let Some(name) = n.attribute("name") {
            types.insert(name.to_string(), read_type(n));
        }
    }
    let mut cal_id = 0;
    let mut next_cal = || {
        cal_id += 1;
        cal_id
    };

    // TM parameters.
    let mut param_bits: HashMap<String, usize> = HashMap::new();
    for n in descendants(doc.root(), "Parameter") {
        let (Some(name), Some(tref)) = (n.attribute("name"), n.attribute("parameterTypeRef")) else { continue };
        let t = types.get(tref).cloned().unwrap_or_default();
        param_bits.insert(name.to_string(), t.bits);
        let mut categ = "N";
        let mut curtx = String::new();
        if !t.enums.is_empty() {
            categ = "S";
            curtx = format!("TXF{:05}", next_cal());
            b.push("txf", &[("TXF_NUMBR", curtx.clone()), ("TXF_DESCR", name.to_string()), ("TXF_RAWFMT", "U".into()), ("TXF_NALIAS", t.enums.len().to_string())], line_of(n));
            for (lo, hi, text) in &t.enums {
                b.push("txp", &[("TXP_NUMBR", curtx.clone()), ("TXP_FROM", lo.to_string()), ("TXP_TO", hi.to_string()), ("TXP_ALTXT", text.clone())], line_of(n));
            }
        } else if !t.points.is_empty() {
            curtx = format!("CAF{:05}", next_cal());
            b.push("caf", &[("CAF_NUMBR", curtx.clone()), ("CAF_DESCR", name.to_string()), ("CAF_ENGFMT", "R".into()), ("CAF_RAWFMT", "U".into()), ("CAF_UNIT", t.unit.clone()), ("CAF_NCURVE", t.points.len().to_string())], line_of(n));
            for (r, c) in &t.points {
                b.push("cap", &[("CAP_NUMBR", curtx.clone()), ("CAP_XVALS", r.clone()), ("CAP_YVALS", c.clone())], line_of(n));
            }
        } else if !t.poly.is_empty() {
            curtx = format!("MCF{:05}", next_cal());
            let mut v = vec![("MCF_IDENT", curtx.clone()), ("MCF_DESCR", name.to_string())];
            for (i, c) in t.poly.iter().enumerate() {
                v.push((["MCF_POL1", "MCF_POL2", "MCF_POL3", "MCF_POL4", "MCF_POL5"][i], c.clone()));
            }
            b.push("mcf", &v, line_of(n));
        }
        b.push(
            "pcf",
            &[
                ("PCF_NAME", name.to_string()),
                ("PCF_DESCR", describe(n)),
                ("PCF_UNIT", t.unit.clone()),
                ("PCF_PTC", t.ptc.to_string()),
                ("PCF_PFC", t.pfc.to_string()),
                ("PCF_CATEG", categ.into()),
                ("PCF_NATUR", "R".into()),
                ("PCF_CURTX", curtx),
            ],
            line_of(n),
        );
    }

    // Packets: concrete containers with a restriction that identifies them.
    let mut spid_counter = 1;
    for n in descendants(doc.root(), "SequenceContainer") {
        if n.attribute("abstract") == Some("true") {
            continue;
        }
        let Some(name) = n.attribute("name") else { continue };
        let mut apid = None;
        let mut service = None;
        let mut subtype = None;
        for c in descendants(n, "Comparison") {
            let (Some(p), Some(v)) = (c.attribute("parameterRef"), c.attribute("value")) else { continue };
            let p = p.to_ascii_uppercase();
            let v = v.trim().to_string();
            if p.contains("APID") {
                apid = Some(v);
            } else if p.contains("SUBTYPE") || p.contains("SUB_TYPE") {
                subtype = Some(v);
            } else if p.contains("SERVICE") || p.ends_with("TYPE") {
                service = Some(v);
            }
        }
        let Some(apid) = apid else {
            problems.push(Problem { root, file: file.to_path_buf(), line: Some(line_of(n)), message: format!("container {name}: no APID comparison, so it isn't a packet Fenix can identify") });
            continue;
        };
        let spid = n.attribute("shortDescription").and_then(|d| d.strip_prefix("SPID ")).map(|s| s.trim().to_string()).unwrap_or_else(|| {
            spid_counter += 1;
            format!("{}", 900_000 + spid_counter)
        });
        b.push(
            "pid",
            &[
                ("PID_TYPE", service.unwrap_or_else(|| "0".into())),
                ("PID_STYPE", subtype.unwrap_or_else(|| "0".into())),
                ("PID_APID", apid),
                ("PID_PI1_VAL", "0".into()),
                ("PID_PI2_VAL", "0".into()),
                ("PID_SPID", spid.clone()),
                ("PID_DESCR", describe(n)),
            ],
            line_of(n),
        );
        b.push("tpcf", &[("TPCF_SPID", spid.clone()), ("TPCF_NAME", name.to_string())], line_of(n));
        let mut at = 0;
        if let Some(list) = child(n, "EntryList") {
            for e in list.children().filter(|e| e.is_element() && e.tag_name().name() == "ParameterRefEntry") {
                let Some(p) = e.attribute("parameterRef") else { continue };
                let bit = location(e).unwrap_or(at);
                at = bit + param_bits.get(p).copied().unwrap_or(0);
                b.push("plf", &[("PLF_NAME", p.to_string()), ("PLF_SPID", spid.clone()), ("PLF_OFFBY", (bit / 8).to_string()), ("PLF_OFFBI", (bit % 8).to_string())], line_of(e));
            }
        }
    }

    // Telecommands.
    let mut cpc_done = std::collections::HashSet::new();
    for n in descendants(doc.root(), "MetaCommand") {
        let Some(name) = n.attribute("name") else { continue };
        let anc = |key: &str| -> Option<String> {
            descendants(n, "AncillaryData").find(|a| a.attribute("name").is_some_and(|x| x.eq_ignore_ascii_case(key))).and_then(|a| a.text()).map(|t| t.trim().to_string()).or_else(|| {
                descendants(n, "ArgumentAssignment").find(|a| a.attribute("argumentName").is_some_and(|x| x.to_ascii_uppercase().contains(key))).and_then(|a| a.attribute("argumentValue")).map(str::to_string)
            })
        };
        let args: Vec<(String, String, Option<String>)> = descendants(n, "Argument")
            .filter_map(|a| Some((a.attribute("name")?.to_string(), a.attribute("argumentTypeRef")?.to_string(), a.attribute("initialValue").map(str::to_string))))
            .collect();
        let mut elements = 0;
        if let Some(cc) = descendants(n, "CommandContainer").next() {
            let mut at = 0;
            for e in descendants(cc, "EntryList").next().into_iter().flat_map(|l| l.children().filter(|e| e.is_element())) {
                match e.tag_name().name() {
                    "ArgumentRefEntry" => {
                        let Some(arg) = e.attribute("argumentRef") else { continue };
                        let Some((_, tref, initial)) = args.iter().find(|(a, _, _)| a == arg) else { continue };
                        let t = types.get(tref).cloned().unwrap_or_default();
                        let bit = location(e).unwrap_or(at);
                        at = bit + t.bits;
                        b.push("cdf", &[("CDF_CNAME", name.to_string()), ("CDF_ELTYPE", "E".into()), ("CDF_ELLEN", t.bits.to_string()), ("CDF_BIT", bit.to_string()), ("CDF_GRPSIZE", "0".into()), ("CDF_PNAME", arg.to_string())], line_of(e));
                        elements += 1;
                        if cpc_done.insert(arg.to_string()) {
                            let mut pafref = String::new();
                            if !t.enums.is_empty() {
                                pafref = format!("PAF{:05}", next_cal());
                                b.push("paf", &[("PAF_NUMBR", pafref.clone()), ("PAF_DESCR", arg.to_string()), ("PAF_RAWFMT", "U".into()), ("PAF_NALIAS", t.enums.len().to_string())], line_of(e));
                                for (lo, _, text) in &t.enums {
                                    b.push("pas", &[("PAS_NUMBR", pafref.clone()), ("PAS_ALTXT", text.clone()), ("PAS_ALVAL", lo.to_string())], line_of(e));
                                }
                            }
                            b.push(
                                "cpc",
                                &[
                                    ("CPC_NAME", arg.to_string()),
                                    ("CPC_DESCR", arg.to_string()),
                                    ("CPC_PTC", t.ptc.to_string()),
                                    ("CPC_PFC", t.pfc.to_string()),
                                    ("CPC_UNIT", t.unit.clone()),
                                    ("CPC_PAFREF", pafref),
                                    ("CPC_DEFVAL", initial.clone().unwrap_or_default()),
                                ],
                                line_of(e),
                            );
                        }
                    }
                    "FixedValueEntry" => {
                        let bits: usize = e.attribute("sizeInBits").and_then(|s| s.parse().ok()).unwrap_or(0);
                        let value = e.attribute("binaryValue").and_then(|v| i64::from_str_radix(v, 16).ok()).unwrap_or(0);
                        let bit = location(e).unwrap_or(at);
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
        b.push(
            "ccf",
            &[
                ("CCF_CNAME", name.to_string()),
                ("CCF_DESCR", n.attribute("shortDescription").map(str::to_string).unwrap_or_else(|| describe(n))),
                ("CCF_DESCR2", child(n, "LongDescription").and_then(|d| d.text()).unwrap_or("").trim().to_string()),
                ("CCF_TYPE", anc("TYPE").or_else(|| anc("SERVICE")).unwrap_or_default()),
                ("CCF_STYPE", anc("SUBTYPE").unwrap_or_default()),
                ("CCF_APID", anc("APID").unwrap_or_default()),
                ("CCF_NPARS", elements.to_string()),
                ("CCF_ACK", anc("ACK").unwrap_or_default()),
            ],
            line_of(n),
        );
    }
    (b.tables, problems)
}

#[cfg(test)]
mod tests {
    use crate::packets::OffsetBase;
    use crate::set::tests::{fixture, temp_root};
    use crate::set::{Kind, MibSet};
    use crate::MibRoot;

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
        std::fs::remove_dir_all(&root.path).ok();
        std::fs::remove_dir_all(&dir.path).ok();
    }
}
