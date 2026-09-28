//! A MIB read against the standards and against itself: APIDs, packet
//! sizes, overlapping bits, type widths, packet identification, the PUS
//! services it uses, error control, calibrations and times. Each problem
//! points at its row, like the reference problems `MibSet` finds.

use std::collections::HashMap;

use fenix_ccsds::packet::IDLE_APID;
use fenix_ccsds::pus::{self, Profile};
use fenix_ccsds::time::TimeFormat;

use crate::detail::{self, CalBody};
use crate::packets::OffsetBase;
use crate::parse::Problem;
use crate::row::Row;
use crate::set::{CalKind, DefRef, Kind, MibSet};
use crate::types::{self, parse_int};

/// Every check, by the name `ccsds.checks` turns it off with.
pub const ALL: &[&str] = &["apid", "size", "overlap", "width", "identification", "pus", "checksum", "calibration", "time"];

pub struct Options<'a> {
    pub disabled: &'a [String],
    pub base: OffsetBase,
    pub profile: &'a Profile,
}

fn at(row: &Row, message: String) -> Problem {
    Problem { root: row.source.root_index, file: row.source.file.clone(), line: Some(row.source.line), message }
}

/// Every standards problem in `set`.
pub fn run(set: &MibSet, o: &Options) -> Vec<Problem> {
    let on = |name: &str| !o.disabled.iter().any(|d| d.eq_ignore_ascii_case(name));
    let mut out = Vec::new();
    let tcs = set.entries(Kind::Telecommand);
    let pkts = set.entries(Kind::TmPacket);

    if on("apid") {
        for e in tcs.iter().chain(pkts) {
            let field = if e.kind == Kind::Telecommand { "CCF_APID" } else { "PID_APID" };
            if let Some(a) = parse_int(e.row.clean(field)) {
                if a > IDLE_APID as i64 {
                    out.push(at(&e.row, format!("{}: APID {a} is over 2047, the largest 11 bits hold (133.0-B)", e.name)));
                } else if a == IDLE_APID as i64 {
                    out.push(at(&e.row, format!("{}: APID 2047 is reserved for idle packets (133.0-B)", e.name)));
                }
            }
        }
    }

    // Per TM packet: sizes and overlaps.
    for (i, e) in pkts.iter().enumerate() {
        let spid = DefRef { kind: Kind::TmPacket, index: i };
        let slots = detail::packet_slots(set, spid);
        let head = crate::packets::params_start(set, spid, o.profile, o.base);
        let mut spans: Vec<(usize, usize, &str)> = Vec::new();
        for s in &slots {
            let Some(p) = s.param else { continue };
            let pcf = &set.get(p).row;
            let t = types::decode(pcf.clean("PCF_PTC"), pcf.clean("PCF_PFC"));
            if on("width") && t.name.starts_with("PTC") {
                out.push(at(pcf, format!("{}: PFC {} isn't defined for PTC {} (ECSS PUS)", s.name, pcf.clean("PCF_PFC"), pcf.clean("PCF_PTC"))));
            }
            if on("width") {
                if let (Some(declared), Some(bits)) = (parse_int(pcf.clean("PCF_WIDTH")).filter(|w| *w > 0), t.bits) {
                    if declared as u32 != bits {
                        out.push(at(pcf, format!("{}: PCF_WIDTH says {declared} bits, PTC/PFC {} is {bits}", s.name, t.name)));
                    }
                }
            }
            let Some(bits) = t.bits.map(|b| b as usize) else { continue };
            let start = (head + s.byte.unwrap_or(0).max(0) as usize) * 8 + s.bit.unwrap_or(0).max(0) as usize;
            let occ = parse_int(&s.occurrences).filter(|n| *n > 1).unwrap_or(1) as usize;
            let plf = set.index().rows_by_field("plf", "PLF_NAME", &s.name, Some(e.root)).into_iter().find(|r| r.clean("PLF_SPID") == e.name);
            let spacing = plf.and_then(|r| parse_int(r.clean("PLF_LGOCC"))).unwrap_or(bits as i64).max(0) as usize;
            let end = start + (occ - 1) * spacing + bits;
            spans.push((start, end, s.name.as_str()));
        }
        if on("size") {
            if let Some(tpcf) = set.index().first_row_by_field("tpcf", "TPCF_SPID", &e.name, Some(e.root)) {
                if let Some(size) = parse_int(tpcf.clean("TPCF_SIZE")).filter(|n| *n > 0) {
                    let crc = if parse_int(e.row.clean("PID_CHECK")) == Some(1) { 2 } else { 0 };
                    let room = (size as usize).saturating_sub(crc) * 8;
                    if let Some((_, end, name)) = spans.iter().max_by_key(|s| s.1) {
                        if *end > room {
                            out.push(at(tpcf, format!("SPID {}: {name} ends at octet {}, past the packet's {size} octets", e.name, end.div_ceil(8))));
                        }
                    }
                }
            }
        }
        if on("overlap") {
            spans.sort();
            for w in spans.windows(2) {
                if w[1].0 < w[0].1 {
                    let row = set.index().rows_by_field("plf", "PLF_NAME", w[1].2, Some(e.root)).into_iter().find(|r| r.clean("PLF_SPID") == e.name).unwrap_or(&e.row);
                    out.push(at(row, format!("SPID {}: {} overlaps {} by {} bits", e.name, w[1].2, w[0].2, w[0].1 - w[1].0)));
                }
            }
        }
    }

    if on("overlap") || on("size") {
        for (i, e) in tcs.iter().enumerate() {
            let tc = DefRef { kind: Kind::Telecommand, index: i };
            let els = detail::tc_elements(set, tc);
            if els.iter().any(|el| el.group > 0) {
                continue;
            }
            let mut spans: Vec<(i64, i64, String)> = els.iter().filter_map(|el| Some((el.bit?, el.bit? + el.bits?, if el.name.is_empty() { "spare".into() } else { el.name.clone() }))).collect();
            spans.sort();
            for w in spans.windows(2) {
                if on("overlap") && w[1].0 < w[0].1 {
                    out.push(at(&e.row, format!("{}: {} overlaps {} by {} bits", e.name, w[1].2, w[0].2, w[0].1 - w[1].0)));
                }
            }
        }
    }

    if on("identification") {
        let mut seen: HashMap<(usize, String, String, String, String, String), &str> = HashMap::new();
        for e in pkts {
            let r = &e.row;
            let key = (e.root, r.clean("PID_TYPE").into(), r.clean("PID_STYPE").into(), r.clean("PID_APID").into(), r.clean("PID_PI1_VAL").into(), r.clean("PID_PI2_VAL").into());
            if let Some(other) = seen.insert(key, &e.name) {
                out.push(at(r, format!("SPID {} and {other} are identified by the same type, subtype, APID and PI values", e.name)));
            }
            let pi = parse_int(r.clean("PID_PI1_VAL")).unwrap_or(0) != 0 || parse_int(r.clean("PID_PI2_VAL")).unwrap_or(0) != 0;
            if pi {
                let has_pic = set.index().rows("pic").iter().any(|p| p.source.root_index == e.root && p.clean("PIC_TYPE") == r.clean("PID_TYPE") && p.clean("PIC_STYPE") == r.clean("PID_STYPE"));
                if !has_pic {
                    out.push(at(r, format!("SPID {}: PI values but no PIC row for {},{} says where to read them", e.name, r.clean("PID_TYPE"), r.clean("PID_STYPE"))));
                }
            }
        }
    }

    if on("pus") {
        for root in 0..set.roots().len() {
            let has_tm = |s: i64, st: i64| pkts.iter().any(|e| e.root == root && parse_int(e.row.clean("PID_TYPE")) == Some(s) && parse_int(e.row.clean("PID_STYPE")) == Some(st));
            let mut asked: HashMap<u8, &Row> = HashMap::new();
            for e in tcs.iter().filter(|e| e.root == root) {
                let ack = parse_int(e.row.clean("CCF_ACK")).unwrap_or(0) as u8;
                for (sub, _) in pus::verification_reports(ack) {
                    asked.entry(sub).or_insert(&e.row);
                }
                if parse_int(e.row.clean("CCF_TYPE")) == Some(17) && parse_int(e.row.clean("CCF_STYPE")) == Some(1) && !has_tm(17, 2) {
                    out.push(at(&e.row, format!("{}: TC(17,1) but no TM(17,2) connection report is defined", e.name)));
                }
            }
            let mut subs: Vec<_> = asked.into_iter().collect();
            subs.sort_by_key(|(s, _)| *s);
            for (sub, row) in subs {
                if !has_tm(1, sub as i64) {
                    out.push(at(row, format!("telecommands ask for TM(1,{sub}) {} but no such packet is defined", pus::subtype_name(1, sub).unwrap_or(""))));
                }
            }
        }
    }

    if on("checksum") {
        let mut by_apid: HashMap<(usize, String), (String, &str)> = HashMap::new();
        for e in pkts {
            let key = (e.root, e.row.clean("PID_APID").to_string());
            let check = e.row.clean("PID_CHECK").to_string();
            match by_apid.get(&key) {
                Some((c, other)) if *c != check => out.push(at(&e.row, format!("SPID {}: PID_CHECK {check} on APID {}, but {other} says {c}", e.name, key.1))),
                None => {
                    by_apid.insert(key, (check, &e.name));
                }
                _ => {}
            }
        }
    }

    if on("calibration") {
        for (i, e) in set.entries(Kind::Calibration).iter().enumerate() {
            let cal = DefRef { kind: Kind::Calibration, index: i };
            match detail::calibration(set, cal) {
                CalBody::Points(p) => {
                    let xs: Vec<f64> = p.iter().filter_map(|(x, _)| x.trim().parse().ok()).collect();
                    if xs.windows(2).any(|w| w[1] <= w[0]) {
                        out.push(at(&e.row, format!("{}: the curve's raw points aren't increasing", e.name)));
                    }
                }
                CalBody::Texts(t) if e.cal == Some(CalKind::Status) => {
                    let mut raws: Vec<&str> = t.iter().map(|(r, _)| r.as_str()).collect();
                    raws.sort_unstable();
                    if raws.windows(2).any(|w| w[0] == w[1]) {
                        out.push(at(&e.row, format!("{}: two texts for the same raw value", e.name)));
                    }
                }
                CalBody::Ranges(r) => {
                    for (lo, hi) in r {
                        if let (Ok(a), Ok(b)) = (lo.trim().parse::<f64>(), hi.trim().parse::<f64>()) {
                            if a > b {
                                out.push(at(&e.row, format!("{}: range {lo}..{hi} has its minimum over its maximum", e.name)));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for (i, e) in set.entries(Kind::TcParam).iter().enumerate() {
            let def = e.row.clean("CPC_DEFVAL");
            let Ok(v) = def.parse::<f64>() else { continue };
            for cal in detail::tc_param_calibrations(set, DefRef { kind: Kind::TcParam, index: i }) {
                if let CalBody::Ranges(r) = detail::calibration(set, cal) {
                    let inside = r.iter().any(|(lo, hi)| lo.trim().parse::<f64>().is_ok_and(|a| a <= v) && hi.trim().parse::<f64>().is_ok_and(|b| v <= b));
                    if !r.is_empty() && !inside {
                        out.push(at(&e.row, format!("{}: default {def} is outside its own range", e.name)));
                    }
                }
            }
        }
    }

    if on("time") && o.profile.tm_time == TimeFormat::None {
        if let Some(e) = pkts.iter().find(|e| e.row.clean("PID_TIME") == "Y") {
            out.push(at(&e.row, format!("SPID {}: PID_TIME says a time is present, but the project's TM time format is none (ccsds.tm_time)", e.name)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set::tests::{temp_root, write};

    fn line(fields: &[&str], n: usize) -> String {
        let mut v: Vec<&str> = fields.to_vec();
        v.resize(n.max(fields.len()), "");
        v.join("\t")
    }

    #[test]
    fn the_standards_checks_find_what_they_say() {
        let r = temp_root("checks");
        write(&r, "ccf", &[line(&["ZTC1", "a", "", "", "N", "", "8", "1", "2047", "", "", "", "", "", "", "", "", "", "", "1"], 21), line(&["ZTC2", "b", "", "", "N", "", "17", "1", "5000"], 21)].join("\n"));
        write(&r, "cdf", &[line(&["ZTC1", "E", "a", "16", "0", "0", "P1"], 10), line(&["ZTC1", "E", "b", "8", "8", "0", "P2"], 10)].join("\n"));
        write(&r, "cpc", &[line(&["P1", "a", "3", "12", "", "", "", "", "PRF1", "", "", "", "99"], 17), line(&["P2", "b", "3", "4"], 17)].join("\n"));
        write(&r, "prf", &line(&["PRF1", "r"], 7));
        write(&r, "prv", "PRF1\t10\t1\n");
        write(&r, "pid", &[line(&["3", "25", "100", "1", "0", "1", "x", "", "", "13", "Y", "", "", "1"], 16), line(&["3", "25", "100", "1", "0", "2", "y", "", "", "13", "Y", "", "", "0"], 16)].join("\n"));
        write(&r, "tpcf", "1\tA\t22\n");
        write(&r, "pcf", &[line(&["N1", "n", "", "", "3", "12", "8"], 24), line(&["N2", "n", "", "", "5", "9"], 24)].join("\n"));
        write(&r, "plf", "N1\t1\t0\t0\nN2\t1\t1\t0\n");
        write(&r, "caf", &line(&["C1", "c"], 8));
        write(&r, "cap", "C1\t10\t0\nC1\t5\t1\n");
        write(&r, "pcf", &[line(&["N1", "n", "", "", "3", "12", "8"], 24), line(&["N2", "n", "", "", "5", "9"], 24), line(&["N3", "n", "", "", "3", "4", "", "", "", "N", "R", "C1"], 24)].join("\n"));
        let set = MibSet::load(vec![r], None);
        let profile = Profile { tm_time: TimeFormat::None, ..Default::default() };
        let got: Vec<String> = run(&set, &Options { disabled: &[], base: OffsetBase::AfterHeaders, profile: &profile }).into_iter().map(|p| p.message).collect();
        let has = |s: &str| got.iter().any(|m| m.contains(s));
        assert!(has("APID 2047 is reserved"), "{got:#?}");
        assert!(has("APID 5000 is over 2047"));
        assert!(has("ZTC1: P2 overlaps P1 by 8 bits"), "{got:#?}");
        assert!(has("PCF_WIDTH says 8 bits"));
        assert!(has("PFC 9 isn't defined for PTC 5"));
        assert!(has("identified by the same type"));
        assert!(has("PI values but no PIC row"));
        assert!(has("TC(17,1) but no TM(17,2)"));
        assert!(has("TM(1,7)"), "ack 1 asks for completion");
        assert!(has("PID_CHECK 0 on APID 100"));
        assert!(has("raw points aren't increasing"));
        assert!(has("minimum over its maximum"));
        assert!(has("default 99 is outside"));
        assert!(has("PID_TIME says a time"));
        let quiet = run(&set, &Options { disabled: &ALL.iter().map(|s| s.to_string()).collect::<Vec<_>>(), base: OffsetBase::AfterHeaders, profile: &profile });
        assert!(quiet.is_empty());
    }
}
