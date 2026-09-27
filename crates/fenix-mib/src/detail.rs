//! What a definition page shows beyond the entry itself, gathered from
//! the set: a telecommand's parameters and their bit layout, a packet's
//! parameters by position, a calibration's points or texts, a TM
//! parameter's limits and the packets carrying it.

use crate::row::Row;
use crate::set::{CalKind, DefRef, Kind, MibSet};
use crate::types::{self, parse_int};

/// One of a telecommand's `CDF` elements.
#[derive(Debug, Clone, PartialEq)]
pub struct TcElement {
    /// Its parameter's name; empty for a spare area.
    pub name: String,
    pub description: String,
    pub bit: Option<i64>,
    pub bits: Option<i64>,
    /// The value it's fixed to, when it is.
    pub fixed: Option<String>,
    /// Its `CPC` definition.
    pub param: Option<DefRef>,
    /// The parameter's type, decoded.
    pub type_name: String,
    pub unit: String,
    pub default: String,
    /// What it may be: "1..8", "OFF ON AUTO".
    pub allowed: String,
    /// The calibration giving `allowed`.
    pub cal: Option<DefRef>,
    /// Its `CDF_GRPSIZE`: how many elements after it repeat, as often as
    /// its value says.
    pub group: usize,
}

pub fn tc_elements(set: &MibSet, tc: DefRef) -> Vec<TcElement> {
    let e = set.get(tc);
    let index = set.index();
    index
        .rows_by_field("cdf", "CDF_CNAME", &e.name, Some(e.root))
        .into_iter()
        .map(|cdf| {
            let name = cdf.clean("CDF_PNAME").to_string();
            let param = (!name.is_empty()).then(|| set.find(Kind::TcParam, e.root, &name)).flatten();
            let cpc = param.map(|p| &set.get(p).row);
            let (allowed, cal) = cpc.map(|c| param_allowed(set, e.root, c)).unwrap_or_default();
            let description = match cdf.clean("CDF_DESCR") {
                "" => cpc.map(|c| c.clean("CPC_DESCR").to_string()).unwrap_or_default(),
                d => d.to_string(),
            };
            let fixed = cdf.clean("CDF_VALUE");
            TcElement {
                description,
                bit: parse_int(cdf.clean("CDF_BIT")),
                bits: parse_int(cdf.clean("CDF_ELLEN")),
                fixed: (!fixed.is_empty()).then(|| fixed.to_string()),
                param,
                type_name: cpc.map(|c| types::decode(c.clean("CPC_PTC"), c.clean("CPC_PFC")).name).unwrap_or_default(),
                unit: cpc.map(|c| c.clean("CPC_UNIT").to_string()).unwrap_or_default(),
                default: cpc.map(|c| c.clean("CPC_DEFVAL").to_string()).unwrap_or_default(),
                allowed,
                cal,
                group: parse_int(cdf.clean("CDF_GRPSIZE")).unwrap_or(0).max(0) as usize,
                name,
            }
        })
        .collect()
}

/// What a TC parameter may be, and the calibration that says so: its
/// status texts, else its range.
fn param_allowed(set: &MibSet, root: usize, cpc: &Row) -> (String, Option<DefRef>) {
    let status = set.find_cal(root, CalKind::Status, cpc.clean("CPC_PAFREF"));
    let range = set.find_cal(root, CalKind::Range, cpc.clean("CPC_PRFREF"));
    let numeric = set.find_cal(root, CalKind::TcNumeric, cpc.clean("CPC_CCAREF"));
    match (status, range) {
        (Some(c), _) | (None, Some(c)) => (set.get(c).cols[1].clone(), Some(c)),
        (None, None) => (String::new(), numeric),
    }
}

/// One parameter in a TM packet.
#[derive(Debug, Clone, PartialEq)]
pub struct PacketSlot {
    pub name: String,
    pub description: String,
    pub byte: Option<i64>,
    pub bit: Option<i64>,
    pub bits: Option<u32>,
    /// How often it repeats in the packet, when it does.
    pub occurrences: String,
    pub param: Option<DefRef>,
}

pub fn packet_slots(set: &MibSet, packet: DefRef) -> Vec<PacketSlot> {
    let e = set.get(packet);
    let mut slots: Vec<PacketSlot> = set
        .index()
        .rows_by_field("plf", "PLF_SPID", &e.name, Some(e.root))
        .into_iter()
        .map(|plf| {
            let name = plf.clean("PLF_NAME").to_string();
            let param = set.find(Kind::TmParam, e.root, &name);
            let pcf = param.map(|p| &set.get(p).row);
            PacketSlot {
                description: pcf.map(|p| p.clean("PCF_DESCR").to_string()).unwrap_or_default(),
                byte: parse_int(plf.clean("PLF_OFFBY")),
                bit: parse_int(plf.clean("PLF_OFFBI")),
                bits: pcf.and_then(|p| types::decode(p.clean("PCF_PTC"), p.clean("PCF_PFC")).bits),
                occurrences: plf.clean("PLF_NBOCC").to_string(),
                param,
                name,
            }
        })
        .collect();
    slots.sort_by_key(|s| (s.byte.unwrap_or(i64::MAX), s.bit.unwrap_or(0)));
    slots
}

/// Where a TM parameter sits in each packet carrying it.
pub fn carried_by(set: &MibSet, param: DefRef) -> Vec<(DefRef, PacketSlot)> {
    let mut out = Vec::new();
    for &packet in set.used_by(param) {
        for slot in packet_slots(set, packet) {
            if slot.param == Some(param) {
                out.push((packet, slot));
            }
        }
    }
    out
}

/// A calibration's content.
#[derive(Debug, Clone, PartialEq)]
pub enum CalBody {
    /// Raw to engineering points, in order.
    Points(Vec<(String, String)>),
    /// Raw values (a range, for a TM text calibration) to texts.
    Texts(Vec<(String, String)>),
    Ranges(Vec<(String, String)>),
    /// Coefficients, lowest order first.
    Coefficients(Vec<String>),
}

pub fn calibration(set: &MibSet, cal: DefRef) -> CalBody {
    let e = set.get(cal);
    let (index, root, id) = (set.index(), Some(e.root), e.name.as_str());
    let pairs = |table: &str, key: &str, a: &str, b: &str| -> Vec<(String, String)> {
        index.rows_by_field(table, key, id, root).into_iter().map(|r| (r.clean(a).to_string(), r.clean(b).to_string())).collect()
    };
    match e.cal.unwrap_or(CalKind::Numeric) {
        CalKind::Numeric => CalBody::Points(pairs("cap", "CAP_NUMBR", "CAP_XVALS", "CAP_YVALS")),
        CalKind::TcNumeric => CalBody::Points(pairs("ccs", "CCS_NUMBR", "CCS_YVALS", "CCS_XVALS")),
        CalKind::Status => CalBody::Texts(pairs("pas", "PAS_NUMBR", "PAS_ALVAL", "PAS_ALTXT")),
        CalKind::Text => CalBody::Texts(
            index
                .rows_by_field("txp", "TXP_NUMBR", id, root)
                .into_iter()
                .map(|r| {
                    let (from, to) = (r.clean("TXP_FROM"), r.clean("TXP_TO"));
                    (if from == to || to.is_empty() { from.to_string() } else { format!("{from}..{to}") }, r.clean("TXP_ALTXT").to_string())
                })
                .collect(),
        ),
        CalKind::Range => CalBody::Ranges(pairs("prv", "PRV_NUMBR", "PRV_MINVAL", "PRV_MAXVAL")),
        CalKind::Polynomial | CalKind::Logarithmic => {
            let prefix = e.row.table.to_uppercase();
            CalBody::Coefficients((1..=5).map(|i| e.row.clean(&format!("{prefix}_POL{i}")).to_string()).take_while(|t| !t.is_empty()).collect())
        }
    }
}

/// One monitoring check on a TM parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limit {
    /// "soft", "hard", "delta", "event", or the MIB's letter.
    pub kind: String,
    pub low: String,
    pub high: String,
    /// Applies only while this parameter has this value.
    pub when: Option<(String, String)>,
}

pub fn limits(set: &MibSet, param: DefRef) -> Vec<Limit> {
    let e = set.get(param);
    set.index()
        .rows_by_field("ocp", "OCP_NAME", &e.name, Some(e.root))
        .into_iter()
        .map(|r| {
            let kind = match r.clean("OCP_TYPE") {
                "S" => "soft",
                "H" => "hard",
                "C" => "delta",
                "E" => "event",
                other => other,
            };
            let rl = r.clean("OCP_RLCHK");
            Limit {
                kind: kind.to_string(),
                low: r.clean("OCP_LVALU").to_string(),
                high: r.clean("OCP_HVALU").to_string(),
                when: (!rl.is_empty()).then(|| (rl.to_string(), r.clean("OCP_VALPAR").to_string())),
            }
        })
        .collect()
}

/// A TM parameter's calibration.
pub fn tm_calibration(set: &MibSet, param: DefRef) -> Option<DefRef> {
    set.entries(Kind::Calibration).iter().enumerate().map(|(index, _)| DefRef { kind: Kind::Calibration, index }).find(|&c| set.used_by(c).contains(&param))
}

/// A TC parameter's calibrations: numeric, status, range.
pub fn tc_param_calibrations(set: &MibSet, param: DefRef) -> Vec<DefRef> {
    let e = set.get(param);
    [("CPC_CCAREF", CalKind::TcNumeric), ("CPC_PAFREF", CalKind::Status), ("CPC_PRFREF", CalKind::Range)]
        .into_iter()
        .filter_map(|(field, cal)| set.find_cal(e.root, cal, e.row.clean(field)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set::tests::fixture;

    #[test]
    fn a_telecommands_elements_carry_their_types_and_what_they_allow() {
        let root = fixture();
        let set = crate::MibSet::load(vec![root.clone()], None);
        let tc = set.find(Kind::Telecommand, 0, "ZTC08101").unwrap();
        let els = tc_elements(&set, tc);
        assert_eq!(els.len(), 4);
        assert_eq!((els[0].fixed.as_deref(), els[0].bit, els[0].bits), (Some("12"), Some(0), Some(8)));
        assert_eq!((els[1].allowed.as_str(), els[1].type_name.as_str()), ("1..8", "uint8"));
        assert_eq!((els[2].allowed.as_str(), els[2].default.as_str()), ("OFF ON AUTO", "AUTO"));
        assert!(els[3].param.is_none());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn packets_calibrations_and_limits_read_from_their_tables() {
        let root = fixture();
        let set = crate::MibSet::load(vec![root.clone()], None);
        let temp = set.find(Kind::TmParam, 0, "NTH00123").unwrap();
        let carried = carried_by(&set, temp);
        assert_eq!(carried.len(), 1);
        assert_eq!((carried[0].1.byte, carried[0].1.bits), (Some(24), Some(16)));
        let cal = tm_calibration(&set, temp).unwrap();
        assert_eq!(calibration(&set, cal), CalBody::Points(vec![("0".into(), "-40".into()), ("4095".into(), "85".into())]));
        let mode = set.find(Kind::TmParam, 0, "NTH00201").unwrap();
        let texts = calibration(&set, tm_calibration(&set, mode).unwrap());
        assert_eq!(texts, CalBody::Texts(vec![("0".into(), "OFF".into()), ("1".into(), "ON".into()), ("2".into(), "AUTO".into())]));
        assert_eq!(limits(&set, temp).iter().map(|l| l.kind.as_str()).collect::<Vec<_>>(), vec!["soft", "hard"]);
        let packet = set.find(Kind::TmPacket, 0, "30211").unwrap();
        assert_eq!(packet_slots(&set, packet).iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["NTH00123", "NTH00201", "NGONE"]);
        std::fs::remove_dir_all(&root.path).ok();
    }
}
