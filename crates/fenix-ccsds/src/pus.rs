//! ECSS PUS on top of the space packet: the TC and TM secondary headers of
//! ECSS-E-ST-70-41C (PUS-C) and ECSS-E-70-41A (PUS-A), the packet error
//! control field, and the names of the standard services.

use crate::bits;
use crate::crc;
use crate::field::{hex, Check, Field, Link};
use crate::packet::{self, Packet, PrimaryHeader};
use crate::time::{self, Epoch, LeapTable, TimeFormat};

pub const STANDARD_C: &str = "ECSS-E-ST-70-41C";
pub const STANDARD_A: &str = "ECSS-E-70-41A";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PusEdition {
    #[default]
    C,
    A,
    /// Plain space packets: no secondary header is read.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pec {
    #[default]
    Ccitt16,
    Iso,
    None,
}

impl Pec {
    pub fn parse(s: &str) -> Result<Pec, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ccitt16" | "crc" | "crc16" => Ok(Pec::Ccitt16),
            "iso" => Ok(Pec::Iso),
            "none" | "" => Ok(Pec::None),
            other => Err(format!("{other}: ccitt16, iso or none")),
        }
    }
}

/// How a mission's packets are laid out beyond the primary header.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub pus: PusEdition,
    pub tm_time: TimeFormat,
    pub epoch: Epoch,
    pub leap: LeapTable,
    pub pec: Pec,
    /// PUS-A TM: the optional packet subcounter and destination ID.
    pub a_tm_counter: bool,
    pub a_tm_destination: bool,
    /// PUS-A TC: the optional source ID.
    pub a_tc_source: bool,
    /// The source ID written into built TCs.
    pub tc_source_id: u16,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            pus: PusEdition::C,
            tm_time: TimeFormat::Cuc { coarse: 4, fine: 2, pfield: false },
            epoch: Epoch::default(),
            leap: LeapTable::default(),
            pec: Pec::Ccitt16,
            a_tm_counter: false,
            a_tm_destination: false,
            a_tc_source: false,
            tc_source_id: 0,
        }
    }
}

/// What a PUS secondary header says.
#[derive(Debug, Clone, PartialEq)]
pub struct Secondary {
    pub service: u8,
    pub subtype: u8,
    /// TC: acknowledgement flags. TM: time reference status.
    pub flags: u8,
    /// TC: source ID. TM: destination ID.
    pub id: Option<u16>,
    /// TM: message type counter.
    pub counter: Option<u16>,
    /// TM: the time, in TAI seconds since 1958.
    pub time: Option<time::Tai>,
    /// Octets it took.
    pub len: usize,
    pub field: Field,
}

/// Octets a TM secondary header takes under `profile`.
pub fn tm_header_len(p: &Profile) -> usize {
    match p.pus {
        PusEdition::C => 7 + p.tm_time.len(),
        PusEdition::A => 3 + p.a_tm_counter as usize + p.a_tm_destination as usize + p.tm_time.len(),
        PusEdition::None => 0,
    }
}

/// Octets a TC secondary header takes under `profile`.
pub fn tc_header_len(p: &Profile) -> usize {
    match p.pus {
        PusEdition::C => 5,
        PusEdition::A => 3 + p.a_tc_source as usize,
        PusEdition::None => 0,
    }
}

fn ack_text(ack: u8) -> String {
    let names = [(8, "acceptance"), (4, "start"), (2, "progress"), (1, "completion")];
    let on: Vec<&str> = names.iter().filter(|(b, _)| ack & b != 0).map(|(_, n)| *n).collect();
    if on.is_empty() { "none".into() } else { on.join(", ") }
}

/// The secondary header of `pkt`, when its flag is set and the profile
/// has PUS.
pub fn decode_secondary(pkt: &Packet, p: &Profile) -> Option<Secondary> {
    if !pkt.header.secondary_header || p.pus == PusEdition::None {
        return None;
    }
    let d = pkt.data();
    let base = 48;
    let std = match p.pus {
        PusEdition::A => STANDARD_A,
        _ => STANDARD_C,
    };
    let link = |h| Link::Standard(std, h);
    let g = |at: usize, n: usize| bits::get(d, at, n);
    let mut fields = Vec::new();
    let (service, subtype, flags, id, counter, time, len);
    if pkt.header.is_tc {
        match p.pus {
            PusEdition::C => {
                let version = g(0, 4)?;
                let ack = g(4, 4)? as u8;
                service = g(8, 8)? as u8;
                subtype = g(16, 8)? as u8;
                let src = g(24, 16)? as u16;
                fields.push(Field::new("PUS version", base, 4, version.to_string(), if version == 2 { "PUS-C" } else { "not PUS-C" }).checked(if version == 2 { Check::Ok("2".into()) } else { Check::Warn(format!("{version}, PUS-C says 2")) }));
                fields.push(Field::new("ack flags", base + 4, 4, format!("{ack:04b}"), ack_text(ack)).linked(link("Acknowledgement flags")));
                fields.push(service_field(base + 8, service, subtype, link("Service type")));
                fields.push(Field::new("source ID", base + 24, 16, format!("0x{src:04X}"), src.to_string()));
                (flags, id, counter, time, len) = (ack, Some(src), None, None, 5);
            }
            _ => {
                let ver = g(1, 3)?;
                let ack = g(4, 4)? as u8;
                service = g(8, 8)? as u8;
                subtype = g(16, 8)? as u8;
                fields.push(Field::new("PUS version", base + 1, 3, ver.to_string(), if ver == 1 { "PUS-A" } else { "not PUS-A" }));
                fields.push(Field::new("ack flags", base + 4, 4, format!("{ack:04b}"), ack_text(ack)));
                fields.push(service_field(base + 8, service, subtype, link("Service Type")));
                let mut l = 3;
                let src = if p.a_tc_source {
                    let s = g(24, 8)? as u16;
                    fields.push(Field::new("source ID", base + 24, 8, s.to_string(), ""));
                    l += 1;
                    Some(s)
                } else {
                    None
                };
                (flags, id, counter, time, len) = (ack, src, None, None, l);
            }
        }
    } else {
        let mut at;
        match p.pus {
            PusEdition::C => {
                let version = g(0, 4)?;
                let trs = g(4, 4)? as u8;
                service = g(8, 8)? as u8;
                subtype = g(16, 8)? as u8;
                let ctr = g(24, 16)? as u16;
                let dst = g(40, 16)? as u16;
                fields.push(Field::new("PUS version", base, 4, version.to_string(), if version == 2 { "PUS-C" } else { "not PUS-C" }).checked(if version == 2 { Check::Ok("2".into()) } else { Check::Warn(format!("{version}, PUS-C says 2")) }));
                fields.push(Field::new("time reference status", base + 4, 4, trs.to_string(), ""));
                fields.push(service_field(base + 8, service, subtype, link("Service type")));
                fields.push(Field::new("message counter", base + 24, 16, format!("0x{ctr:04X}"), ctr.to_string()));
                fields.push(Field::new("destination ID", base + 40, 16, format!("0x{dst:04X}"), if dst == 0 { "0 (ground)".into() } else { dst.to_string() }));
                (flags, id, counter) = (trs, Some(dst), Some(ctr));
                at = 7;
            }
            _ => {
                let ver = g(1, 3)?;
                service = g(8, 8)? as u8;
                subtype = g(16, 8)? as u8;
                fields.push(Field::new("PUS version", base + 1, 3, ver.to_string(), if ver == 1 { "PUS-A" } else { "not PUS-A" }));
                fields.push(service_field(base + 8, service, subtype, link("Service Type")));
                at = 3;
                let ctr = if p.a_tm_counter {
                    let c = g(at * 8, 8)? as u16;
                    fields.push(Field::new("packet subcounter", base + at * 8, 8, c.to_string(), ""));
                    at += 1;
                    Some(c)
                } else {
                    None
                };
                let dst = if p.a_tm_destination {
                    let x = g(at * 8, 8)? as u16;
                    fields.push(Field::new("destination ID", base + at * 8, 8, x.to_string(), ""));
                    at += 1;
                    Some(x)
                } else {
                    None
                };
                (flags, id, counter) = (0, dst, ctr);
            }
        }
        let t = time::decode(d.get(at..)?, p.tm_time, p.epoch, &p.leap, base + at * 8);
        time = t.as_ref().map(|t| t.tai);
        if let Some(t) = t {
            fields.push(t.field);
            at += t.len;
        } else if !p.tm_time.is_empty() {
            return None;
        }
        len = at;
    }
    let label = if pkt.header.is_tc { "TC" } else { "TM" };
    let edition = match p.pus {
        PusEdition::A => "PUS-A",
        _ => "PUS-C",
    };
    let mut field = Field::group(format!("{edition} {label} header"), fields);
    field.raw = hex(&d[..len.min(d.len())]);
    Some(Secondary { service, subtype, flags, id, counter, time, len, field })
}

fn service_field(at: usize, service: u8, subtype: u8, link: Link) -> Field {
    Field::group(
        "service",
        vec![
            Field::new("service type", at, 8, service.to_string(), service_name(service).unwrap_or(if service >= 128 { "mission specific" } else { "unknown" })),
            Field::new("subtype", at + 8, 8, subtype.to_string(), subtype_name(service, subtype).unwrap_or("")),
        ],
    )
    .with_value(format!("{service},{subtype}"))
    .linked(link)
}

/// Checks the packet error control at the end of `pkt`.
pub fn pec_field(pkt: &Packet, pec: Pec) -> Option<Field> {
    let n = pkt.bytes.len();
    if n < 8 || pec == Pec::None {
        return None;
    }
    let at = (n - 2) * 8;
    let raw = u16::from_be_bytes([pkt.bytes[n - 2], pkt.bytes[n - 1]]);
    let (name, check) = match pec {
        Pec::Ccitt16 => {
            let want = crc::ccitt16(&pkt.bytes[..n - 2]);
            ("CRC-16-CCITT", if want == raw { Check::Ok("matches".into()) } else { Check::Bad(format!("computed {want:04X}")) })
        }
        Pec::Iso => ("ISO checksum", if crc::iso_checksum_ok(&pkt.bytes) { Check::Ok("adds up".into()) } else { Check::Bad("doesn't add up".into()) }),
        Pec::None => unreachable!(),
    };
    Some(Field::new("packet error control", at, 16, format!("{raw:04X}"), name).checked(check).linked(Link::Standard(STANDARD_C, "Packet error control")))
}

/// The error control a packet ending in `bytes` needs.
pub fn pec_bytes(bytes: &[u8], pec: Pec) -> Vec<u8> {
    match pec {
        Pec::Ccitt16 => crc::ccitt16(bytes).to_be_bytes().to_vec(),
        Pec::Iso => crc::iso_checksum(bytes).to_vec(),
        Pec::None => Vec::new(),
    }
}

/// The TC secondary header for a built telecommand.
pub fn encode_tc_header(p: &Profile, service: u8, subtype: u8, ack: u8) -> Vec<u8> {
    let mut out = Vec::new();
    match p.pus {
        PusEdition::C => {
            bits::put(&mut out, 0, 4, 2);
            bits::put(&mut out, 4, 4, ack as u64);
            bits::put(&mut out, 8, 8, service as u64);
            bits::put(&mut out, 16, 8, subtype as u64);
            bits::put(&mut out, 24, 16, p.tc_source_id as u64);
        }
        PusEdition::A => {
            bits::put(&mut out, 1, 3, 1);
            bits::put(&mut out, 4, 4, ack as u64);
            bits::put(&mut out, 8, 8, service as u64);
            bits::put(&mut out, 16, 8, subtype as u64);
            if p.a_tc_source {
                bits::put(&mut out, 24, 8, p.tc_source_id as u64);
            }
        }
        PusEdition::None => {}
    }
    out
}

/// A whole TC packet: header, PUS header, application data, error control.
pub fn build_tc(p: &Profile, apid: u16, seq: u16, service: u8, subtype: u8, ack: u8, app_data: &[u8]) -> Packet {
    let mut data = encode_tc_header(p, service, subtype, ack);
    data.extend_from_slice(app_data);
    let pec_len = match p.pec {
        Pec::None => 0,
        _ => 2,
    };
    let header = PrimaryHeader { is_tc: true, secondary_header: p.pus != PusEdition::None, apid, seq_flags: 3, seq_count: seq, ..Default::default() };
    let mut pkt = Packet::build(header, &{
        let mut d = data.clone();
        d.extend(std::iter::repeat_n(0, pec_len));
        d
    });
    let n = pkt.bytes.len();
    let pec = pec_bytes(&pkt.bytes[..n - pec_len], p.pec);
    pkt.bytes[n - pec_len..].copy_from_slice(&pec);
    pkt
}

/// Decodes a packet down to its PUS layer: the primary header, the
/// secondary header, what's left as user data, and the error control.
pub fn decode(bytes: &[u8], p: &Profile) -> Option<(Packet, Option<Secondary>, Field)> {
    let (pkt, _) = Packet::parse(bytes)?;
    let mut children = vec![packet::header_field(bytes)?];
    let sec = decode_secondary(&pkt, p);
    let pec = pec_field(&pkt, p.pec);
    let head = 6 + sec.as_ref().map(|s| s.len).unwrap_or(0);
    let tail = if pec.is_some() { 2 } else { 0 };
    if let Some(s) = &sec {
        children.push(s.field.clone());
    }
    let user = pkt.bytes.get(head..pkt.bytes.len().saturating_sub(tail)).unwrap_or(&[]);
    if !user.is_empty() {
        children.push(Field::new("user data", head * 8, user.len() * 8, hex(user), format!("{} octets", user.len())));
    }
    if let Some(f) = pec {
        children.push(f);
    }
    let title = match &sec {
        Some(s) => format!("{}({},{})", if pkt.header.is_tc { "TC" } else { "TM" }, s.service, s.subtype),
        None => format!("{} packet", if pkt.header.is_tc { "TC" } else { "TM" }),
    };
    let mut root = Field::group(title, children);
    root.raw = format!("{} octets", pkt.bytes.len());
    root.value = format!("APID 0x{:03X}", pkt.header.apid);
    Some((pkt, sec, root))
}

/// A standard service's name.
pub fn service_name(service: u8) -> Option<&'static str> {
    Some(match service {
        1 => "request verification",
        2 => "device access",
        3 => "housekeeping",
        4 => "parameter statistics reporting",
        5 => "event reporting",
        6 => "memory management",
        8 => "function management",
        9 => "time management",
        11 => "time-based scheduling",
        12 => "on-board monitoring",
        13 => "large packet transfer",
        14 => "real-time forwarding control",
        15 => "on-board storage and retrieval",
        17 => "test",
        18 => "on-board operations procedure",
        19 => "event-action",
        20 => "on-board parameter management",
        21 => "request sequencing",
        22 => "position-based scheduling",
        23 => "file management",
        _ => return None,
    })
}

/// A standard subtype's name, where Fenix knows it.
pub fn subtype_name(service: u8, subtype: u8) -> Option<&'static str> {
    Some(match (service, subtype) {
        (1, 1) => "successful acceptance verification report",
        (1, 2) => "failed acceptance verification report",
        (1, 3) => "successful start of execution verification report",
        (1, 4) => "failed start of execution verification report",
        (1, 5) => "successful progress of execution verification report",
        (1, 6) => "failed progress of execution verification report",
        (1, 7) => "successful completion of execution verification report",
        (1, 8) => "failed completion of execution verification report",
        (1, 10) => "failed routing verification report",
        (3, 1) => "create a housekeeping parameter report structure",
        (3, 2) => "create a diagnostic parameter report structure",
        (3, 3) => "delete housekeeping parameter report structures",
        (3, 4) => "delete diagnostic parameter report structures",
        (3, 5) => "enable the periodic generation of housekeeping parameter reports",
        (3, 6) => "disable the periodic generation of housekeeping parameter reports",
        (3, 7) => "enable the periodic generation of diagnostic parameter reports",
        (3, 8) => "disable the periodic generation of diagnostic parameter reports",
        (3, 9) => "report housekeeping parameter report structures",
        (3, 10) => "housekeeping parameter report structure report",
        (3, 25) => "housekeeping parameter report",
        (3, 26) => "diagnostic parameter report",
        (3, 27) => "generate a one shot report for housekeeping parameter report structures",
        (3, 28) => "generate a one shot report for diagnostic parameter report structures",
        (5, 1) => "informative event report",
        (5, 2) => "low severity anomaly report",
        (5, 3) => "medium severity anomaly report",
        (5, 4) => "high severity anomaly report",
        (5, 5) => "enable the report generation of event definitions",
        (5, 6) => "disable the report generation of event definitions",
        (5, 7) => "report the list of disabled event definitions",
        (5, 8) => "disabled event definitions list report",
        (6, 2) => "load object memory data",
        (6, 5) => "dump object memory data",
        (6, 6) => "dumped object memory data report",
        (6, 9) => "check object memory data",
        (6, 10) => "checked object memory data report",
        (8, 1) => "perform a function",
        (9, 1) => "set the time report generation rate",
        (9, 2) => "CUC time report",
        (9, 3) => "CDS time report",
        (11, 1) => "enable the time-based schedule execution function",
        (11, 2) => "disable the time-based schedule execution function",
        (11, 3) => "reset the time-based schedule",
        (11, 4) => "insert activities into the time-based schedule",
        (11, 5) => "delete time-based scheduled activities identified by request identifier",
        (12, 1) => "enable parameter monitoring definitions",
        (12, 2) => "disable parameter monitoring definitions",
        (13, 1) => "first downlink part report",
        (13, 2) => "intermediate downlink part report",
        (13, 3) => "last downlink part report",
        (13, 9) => "first uplink part",
        (13, 10) => "intermediate uplink part",
        (13, 11) => "last uplink part",
        (17, 1) => "are-you-alive connection test",
        (17, 2) => "are-you-alive connection report",
        (17, 3) => "perform an on-board connection test",
        (17, 4) => "on-board connection test report",
        (19, 1) => "add event-action definitions",
        (19, 2) => "delete event-action definitions",
        (19, 4) => "enable event-action definitions",
        (19, 5) => "disable event-action definitions",
        (20, 1) => "report parameter values",
        (20, 2) => "parameter value report",
        (20, 3) => "set parameter values",
        (23, 1) => "create a file",
        (23, 2) => "delete a file",
        (23, 3) => "report the attributes of a file",
        (23, 4) => "file attribute report",
        _ => return None,
    })
}

/// `8,1 function management · perform a function`.
pub fn describe(service: u8, subtype: u8) -> String {
    match (service_name(service), subtype_name(service, subtype)) {
        (Some(s), Some(t)) => format!("{service},{subtype} {s} · {t}"),
        (Some(s), None) => format!("{service},{subtype} {s}"),
        _ => format!("{service},{subtype}"),
    }
}

/// The successful verification reports an ack field asks for, as
/// (service 1 subtype, stage). Failures are reported whatever the flags.
pub fn verification_reports(ack: u8) -> Vec<(u8, &'static str)> {
    [(8, 1, "acceptance"), (4, 3, "start"), (2, 5, "progress"), (1, 7, "completion")].into_iter().filter(|(bit, _, _)| ack & bit != 0).map(|(_, sub, name)| (sub, name)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TM: [u8; 29] = [
        0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16, 0x20, 0x03, 0x19, 0x00, 0x42, 0x00, 0x00, 0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00, 0x00, 0x01, 0x01, 0x0B, 0xB8, 0x0A, 0x28,
        0x02, 0x60, 0xE5,
    ];

    #[test]
    fn the_example_tm_decodes_through_pus_c() {
        let (pkt, sec, root) = decode(&TM, &Profile::default()).unwrap();
        let sec = sec.unwrap();
        assert_eq!((sec.service, sec.subtype, sec.counter, sec.id, sec.len), (3, 25, Some(0x42), Some(0), 13));
        assert_eq!(root.name, "TM(3,25)");
        assert!(!root.any_bad(), "{root:#?}");
        assert_eq!(root.find("user data").unwrap().raw, "00 01 01 0B B8 0A 28 02");
        assert!(root.find("time (CUC 4+2)").unwrap().value.starts_with("2026-09-27 14:32:05.500"));
        assert_eq!(pkt.header.seq_count, 0x123);
        let mut bad = TM;
        bad[23] ^= 1;
        assert!(decode(&bad, &Profile::default()).unwrap().2.any_bad());
    }

    #[test]
    fn a_built_tc_is_the_example_tc() {
        let pkt = build_tc(&Profile::default(), 0x3F2, 7, 8, 1, 0b1001, &[12, 3, 2]);
        assert_eq!(pkt.bytes, vec![0x1B, 0xF2, 0xC0, 0x07, 0x00, 0x09, 0x29, 0x08, 0x01, 0x00, 0x00, 0x0C, 0x03, 0x02, 0x3F, 0xA3]);
        let (_, sec, root) = decode(&pkt.bytes, &Profile::default()).unwrap();
        assert_eq!(sec.unwrap().flags, 0b1001);
        assert!(!root.any_bad());
        assert_eq!(root.find("ack flags").unwrap().value, "acceptance, completion");
    }

    #[test]
    fn pus_a_and_iso_checksums() {
        let p = Profile { pus: PusEdition::A, pec: Pec::Iso, a_tc_source: true, tc_source_id: 5, ..Default::default() };
        let pkt = build_tc(&p, 0x10, 1, 17, 1, 0b1001, &[]);
        let (_, sec, root) = decode(&pkt.bytes, &p).unwrap();
        let sec = sec.unwrap();
        assert_eq!((sec.service, sec.subtype, sec.id, sec.len), (17, 1, Some(5), 4));
        assert!(!root.any_bad(), "{root:#?}");
        assert_eq!(describe(17, 1), "17,1 test · are-you-alive connection test");
        assert_eq!(describe(200, 1), "200,1");
    }
}
