//! Transfer frames: TM (132.0-B), TC (232.0-B), AOS (732.0-B) and USLP
//! (732.1-B), with the CLCW (232.0-B's communications link control word)
//! an OCF carries and the frame error control field.

use crate::bits;
use crate::crc;
use crate::field::{hex, hex_tight, Check, Field, Link};

pub const TM_STANDARD: &str = "CCSDS 132.0-B";
pub const TC_STANDARD: &str = "CCSDS 232.0-B";
pub const AOS_STANDARD: &str = "CCSDS 732.0-B";
pub const USLP_STANDARD: &str = "CCSDS 732.1-B";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FrameKind {
    #[default]
    Tm,
    Tc,
    Aos,
    Uslp,
}

impl FrameKind {
    pub fn parse(s: &str) -> Result<FrameKind, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tm" => Ok(FrameKind::Tm),
            "tc" => Ok(FrameKind::Tc),
            "aos" => Ok(FrameKind::Aos),
            "uslp" => Ok(FrameKind::Uslp),
            other => Err(format!("{other}: tm, tc, aos or uslp")),
        }
    }
}

/// How a mission's frames are built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameProfile {
    pub kind: FrameKind,
    /// Octets in a frame (without the sync marker).
    pub length: usize,
    pub asm: bool,
    pub randomized: bool,
    /// Reed-Solomon interleave depth; 0 when there's none.
    pub rs_depth: usize,
    /// Virtual fill per code word.
    pub rs_pad: usize,
    pub ocf: bool,
    pub fecf: bool,
    /// AOS: the frame header error control field.
    pub aos_fhec: bool,
    /// AOS: the insert zone's length.
    pub aos_insert: usize,
    /// TC: whether frames carry a segment header.
    pub tc_segment_header: bool,
    pub vc_names: Vec<(u8, String)>,
}

impl Default for FrameProfile {
    fn default() -> Self {
        FrameProfile {
            kind: FrameKind::Tm,
            length: 1115,
            asm: true,
            randomized: false,
            rs_depth: 0,
            rs_pad: 0,
            ocf: true,
            fecf: false,
            aos_fhec: false,
            aos_insert: 0,
            tc_segment_header: true,
            vc_names: Vec::new(),
        }
    }
}

impl FrameProfile {
    pub fn vc_name(&self, vc: u8) -> Option<&str> {
        self.vc_names.iter().find(|(v, _)| *v == vc).map(|(_, n)| n.as_str())
    }
}

/// What a frame header says, for reassembling packets.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FrameInfo {
    pub scid: u16,
    pub vcid: u8,
    pub mc_count: Option<u64>,
    pub vc_count: u64,
    /// The first packet header's offset in the data field; `None` when no
    /// packet starts here, `Some(u16::MAX)` for idle data.
    pub first_header: Option<u16>,
    /// The data field's range in the frame.
    pub data: std::ops::Range<usize>,
    pub ocf: Option<u32>,
}

fn name_vc(vc: u8, p: &FrameProfile) -> String {
    match p.vc_name(vc) {
        Some(n) => format!("VC {vc} ({n})"),
        None => format!("VC {vc}"),
    }
}

fn fecf_field(bytes: &[u8], std: &'static str) -> Option<Field> {
    let n = bytes.len();
    if n < 4 {
        return None;
    }
    let raw = u16::from_be_bytes([bytes[n - 2], bytes[n - 1]]);
    let want = crc::ccitt16(&bytes[..n - 2]);
    Some(
        Field::new("FECF", (n - 2) * 8, 16, format!("{raw:04X}"), "CRC-16")
            .checked(if raw == want { Check::Ok("matches".into()) } else { Check::Bad(format!("computed {want:04X}")) })
            .linked(Link::Standard(std, "Frame Error Control Field")),
    )
}

/// The TC frame a CLTU's data starts with: cut to the length its header
/// gives (the fill after it dropped), then decoded with `p`. Also says
/// what's wrong with it as a frame the spacecraft would take -- too short
/// for its length, or a failed FECF.
pub fn tc_frame(data: &[u8], p: &FrameProfile) -> Option<(Field, FrameInfo, Vec<u8>, Option<String>)> {
    let len = bits::get(data, 22, 10)? as usize + 1;
    let frame = data[..len.min(data.len())].to_vec();
    let p = FrameProfile { kind: FrameKind::Tc, ..p.clone() };
    let (field, info) = decode_tc(&frame, &p)?;
    let problem = if frame.len() < len {
        Some(format!("TC frame says {len} octets, the CLTU carried {}", frame.len()))
    } else {
        field.walk().into_iter().find_map(|(_, f)| match &f.check {
            Some(Check::Bad(why)) => Some(format!("TC frame {} {}: {why}", f.name, f.raw)),
            _ => None,
        })
    };
    Some((field, info, frame, problem))
}

/// Decodes a frame (after any sync marker, de-randomized, corrected).
pub fn decode(bytes: &[u8], p: &FrameProfile) -> Option<(Field, FrameInfo)> {
    match p.kind {
        FrameKind::Tm => decode_tm(bytes, p),
        FrameKind::Tc => decode_tc(bytes, p),
        FrameKind::Aos => decode_aos(bytes, p),
        FrameKind::Uslp => decode_uslp(bytes, p),
    }
}

fn decode_tm(b: &[u8], p: &FrameProfile) -> Option<(Field, FrameInfo)> {
    let g = |at, n| bits::get(b, at, n);
    let link = |h| Link::Standard(TM_STANDARD, h);
    let version = g(0, 2)?;
    let scid = g(2, 10)? as u16;
    let vcid = g(12, 3)? as u8;
    let ocf_flag = g(15, 1)? == 1;
    let mc = g(16, 8)?;
    let vc = g(24, 8)?;
    let sec = g(32, 1)? == 1;
    let sync = g(33, 1)?;
    let fhp = g(37, 11)? as u16;
    let mut header = vec![
        Field::new("version", 0, 2, version.to_string(), if version == 0 { "TM" } else { "not TM" }).checked(if version == 0 { Check::Ok("0".into()) } else { Check::Bad(format!("{version}, TM frames are 0")) }),
        Field::new("SCID", 2, 10, format!("0x{scid:03X}"), scid.to_string()).linked(link("Spacecraft Identifier")),
        Field::new("VCID", 12, 3, vcid.to_string(), name_vc(vcid, p)).linked(link("Virtual Channel Identifier")),
        Field::new("OCF flag", 15, 1, (ocf_flag as u8).to_string(), if ocf_flag { "OCF present" } else { "no OCF" }),
        Field::new("MC count", 16, 8, format!("0x{mc:02X}"), mc.to_string()).linked(link("Master Channel Frame Count")),
        Field::new("VC count", 24, 8, format!("0x{vc:02X}"), vc.to_string()).linked(link("Virtual Channel Frame Count")),
        Field::new("secondary header", 32, 1, (sec as u8).to_string(), if sec { "present" } else { "absent" }),
        Field::new("sync flag", 33, 1, sync.to_string(), if sync == 0 { "packets" } else { "VCA_SDU" }),
        Field::new(
            "first header pointer",
            37,
            11,
            format!("0x{fhp:03X}"),
            match fhp {
                0x7FF => "no packet starts here".to_string(),
                0x7FE => "idle data only".to_string(),
                n => format!("packet starts at {n}"),
            },
        )
        .linked(link("First Header Pointer")),
    ];
    let mut data_start = 6;
    if sec {
        let len = g(50, 6)? as usize + 1;
        header.push(Field::new("secondary header", 48, (len + 1) * 8, hex(b.get(6..7 + len)?), format!("{len} octets")));
        data_start += 1 + len;
    }
    let mut end = b.len();
    let mut tail = Vec::new();
    let fecf = if p.fecf { fecf_field(b, TM_STANDARD) } else { None };
    if fecf.is_some() {
        end -= 2;
    }
    let mut ocf = None;
    if ocf_flag && end >= data_start + 4 {
        let word = u32::from_be_bytes(b[end - 4..end].try_into().ok()?);
        tail.push(clcw_field(word, (end - 4) * 8));
        ocf = Some(word);
        end -= 4;
    }
    let mut children = vec![Field::group("TM frame header", header)];
    children.push(Field::new("data field", data_start * 8, (end - data_start) * 8, format!("{} octets", end - data_start), ""));
    children.extend(tail);
    children.extend(fecf);
    let first_header = match fhp {
        0x7FF => None,
        0x7FE => Some(u16::MAX),
        n => Some(n),
    };
    let info = FrameInfo { scid, vcid, mc_count: Some(mc), vc_count: vc, first_header, data: data_start..end, ocf };
    Some((Field::group(format!("TM frame · {}", name_vc(vcid, p)), children), info))
}

fn decode_tc(b: &[u8], p: &FrameProfile) -> Option<(Field, FrameInfo)> {
    let g = |at, n| bits::get(b, at, n);
    let link = |h| Link::Standard(TC_STANDARD, h);
    let version = g(0, 2)?;
    let bypass = g(2, 1)?;
    let ctrl = g(3, 1)?;
    let scid = g(6, 10)? as u16;
    let vcid = g(16, 6)? as u8;
    let len = g(22, 10)? as usize + 1;
    let seq = g(32, 8)?;
    let length_check = if len == b.len() { Check::Ok(format!("{len} octets")) } else { Check::Warn(format!("says {len} octets, {} here", b.len())) };
    let mut header = vec![
        Field::new("version", 0, 2, version.to_string(), if version == 0 { "TC" } else { "not TC" }),
        Field::new("bypass", 2, 1, bypass.to_string(), if bypass == 1 { "Type-B (bypass)" } else { "Type-A (sequence controlled)" }).linked(link("Bypass Flag")),
        Field::new("control command", 3, 1, ctrl.to_string(), if ctrl == 1 { "control command" } else { "data" }),
        Field::new("SCID", 6, 10, format!("0x{scid:03X}"), scid.to_string()).linked(link("Spacecraft Identifier")),
        Field::new("VCID", 16, 6, vcid.to_string(), name_vc(vcid, p)),
        Field::new("frame length", 22, 10, (len - 1).to_string(), format!("{len} octets")).checked(length_check),
        Field::new("frame sequence", 32, 8, seq.to_string(), ""),
    ];
    let mut start = 5;
    if p.tc_segment_header && ctrl == 0 {
        let flags = g(40, 2)?;
        let map = g(42, 6)?;
        header.push(Field::new(
            "segment header",
            40,
            8,
            format!("{:02X}", b.get(5)?),
            format!("{} · MAP {map}", match flags { 3 => "unsegmented", 1 => "first", 2 => "last", _ => "continuing" }),
        ));
        start = 6;
    }
    let end = if p.fecf { b.len().min(len).saturating_sub(2) } else { b.len().min(len) };
    let mut children = vec![Field::group("TC frame header", header)];
    children.push(Field::new("data field", start * 8, end.saturating_sub(start) * 8, format!("{} octets", end.saturating_sub(start)), ""));
    if p.fecf {
        children.extend(fecf_field(&b[..len.min(b.len())], TC_STANDARD));
    }
    let info = FrameInfo { scid, vcid, mc_count: None, vc_count: seq, first_header: Some(0), data: start..end, ocf: None };
    Some((Field::group(format!("TC frame · {}", name_vc(vcid, p)), children), info))
}

fn decode_aos(b: &[u8], p: &FrameProfile) -> Option<(Field, FrameInfo)> {
    let g = |at, n| bits::get(b, at, n);
    let link = |h| Link::Standard(AOS_STANDARD, h);
    let version = g(0, 2)?;
    let scid = g(2, 8)? as u16;
    let vcid = g(10, 6)? as u8;
    let count = g(16, 24)?;
    let replay = g(40, 1)?;
    let mut header = vec![
        Field::new("version", 0, 2, version.to_string(), if version == 1 { "AOS" } else { "not AOS" }).checked(if version == 1 { Check::Ok("1".into()) } else { Check::Bad(format!("{version}, AOS frames are 1")) }),
        Field::new("SCID", 2, 8, format!("0x{scid:02X}"), scid.to_string()).linked(link("Spacecraft Identifier")),
        Field::new("VCID", 10, 6, vcid.to_string(), name_vc(vcid, p)),
        Field::new("VC frame count", 16, 24, format!("0x{count:06X}"), count.to_string()),
        Field::new("replay", 40, 1, replay.to_string(), if replay == 1 { "replay" } else { "realtime" }),
    ];
    let mut at = 6;
    if p.aos_fhec {
        header.push(Field::new("FHEC", 48, 16, hex_tight(b.get(6..8)?), "Reed-Solomon (10,6), not checked"));
        at += 2;
    }
    if p.aos_insert > 0 {
        header.push(Field::new("insert zone", at * 8, p.aos_insert * 8, hex(b.get(at..at + p.aos_insert)?), ""));
        at += p.aos_insert;
    }
    let fhp = g(at * 8 + 5, 11)? as u16;
    header.push(
        Field::new("first header pointer", at * 8, 16, format!("0x{fhp:03X}"), match fhp {
            0x7FF => "no packet starts here".to_string(),
            0x7FE => "idle data only".to_string(),
            n => format!("packet starts at {n}"),
        })
        .linked(link("First Header Pointer")),
    );
    at += 2;
    let mut end = b.len();
    let fecf = if p.fecf { fecf_field(b, AOS_STANDARD) } else { None };
    if fecf.is_some() {
        end -= 2;
    }
    let mut ocf = None;
    let mut tail = Vec::new();
    if p.ocf && end >= at + 4 {
        let word = u32::from_be_bytes(b[end - 4..end].try_into().ok()?);
        tail.push(clcw_field(word, (end - 4) * 8));
        ocf = Some(word);
        end -= 4;
    }
    let mut children = vec![Field::group("AOS frame header", header)];
    children.push(Field::new("data field", at * 8, (end - at) * 8, format!("{} octets", end - at), ""));
    children.extend(tail);
    children.extend(fecf);
    let first_header = match fhp {
        0x7FF => None,
        0x7FE => Some(u16::MAX),
        n => Some(n),
    };
    Some((Field::group(format!("AOS frame · {}", name_vc(vcid, p)), children), FrameInfo { scid, vcid, mc_count: None, vc_count: count, first_header, data: at..end, ocf }))
}

fn decode_uslp(b: &[u8], p: &FrameProfile) -> Option<(Field, FrameInfo)> {
    let g = |at, n| bits::get(b, at, n);
    let link = |h| Link::Standard(USLP_STANDARD, h);
    let version = g(0, 4)?;
    let scid = g(4, 16)? as u16;
    let vcid = g(21, 6)? as u8;
    let map = g(27, 4)?;
    let eofph = g(31, 1)? == 1;
    let mut header = vec![
        Field::new("version", 0, 4, format!("{version:04b}"), if version == 0b1100 { "USLP" } else { "not USLP" }).checked(if version == 0b1100 { Check::Ok("1100".into()) } else { Check::Bad("USLP frames are 1100".into()) }),
        Field::new("SCID", 4, 16, format!("0x{scid:04X}"), scid.to_string()).linked(link("Spacecraft Identifier")),
        Field::new("VCID", 21, 6, vcid.to_string(), name_vc(vcid, p)),
        Field::new("MAP ID", 27, 4, map.to_string(), ""),
    ];
    if eofph {
        header.push(Field::new("truncated header", 31, 1, "1", "4-octet truncated frame"));
        return Some((Field::group("USLP frame (truncated)", header), FrameInfo { scid, vcid, data: 4..b.len(), ..Default::default() }));
    }
    let len = g(32, 16)? as usize + 1;
    let ocf_flag = g(51, 1)? == 1;
    let count_len = g(52, 3)? as usize;
    let count = g(55, count_len * 8)?;
    header.push(Field::new("frame length", 32, 16, (len - 1).to_string(), format!("{len} octets")));
    header.push(Field::new("VC count", 55, count_len * 8, count.to_string(), format!("{count_len} octets")));
    let mut at = 7 + count_len;
    let rule = g(at * 8, 3)?;
    let proto = g(at * 8 + 3, 5)?;
    header.push(Field::new("TFDZ construction rule", at * 8, 3, rule.to_string(), if rule == 0 { "packets spanning frames" } else { "" }));
    header.push(Field::new("protocol ID", at * 8 + 3, 5, proto.to_string(), if proto == 0 { "space packets" } else { "" }));
    let mut fhp = None;
    at += 1;
    if rule <= 2 {
        let v = g(at * 8, 16)? as u16;
        header.push(Field::new("first header pointer", at * 8, 16, format!("0x{v:04X}"), if v == 0xFFFF { "no packet starts here".to_string() } else { format!("packet starts at {v}") }));
        fhp = (v != 0xFFFF).then_some(v);
        at += 2;
    }
    let mut end = len.min(b.len());
    let fecf = if p.fecf { fecf_field(&b[..end], USLP_STANDARD) } else { None };
    if fecf.is_some() {
        end -= 2;
    }
    let mut ocf = None;
    let mut tail = Vec::new();
    if ocf_flag && end >= at + 4 {
        let word = u32::from_be_bytes(b[end - 4..end].try_into().ok()?);
        tail.push(clcw_field(word, (end - 4) * 8));
        ocf = Some(word);
        end -= 4;
    }
    let mut children = vec![Field::group("USLP frame header", header)];
    children.push(Field::new("data zone", at * 8, (end.saturating_sub(at)) * 8, format!("{} octets", end.saturating_sub(at)), ""));
    children.extend(tail);
    children.extend(fecf);
    Some((Field::group(format!("USLP frame · {}", name_vc(vcid, p)), children), FrameInfo { scid, vcid, mc_count: None, vc_count: count, first_header: fhp, data: at..end, ocf }))
}

/// The CLCW, 232.0-B's report of the uplink's state, from its 32 bits.
pub fn clcw_field(word: u32, at: usize) -> Field {
    let b = word.to_be_bytes();
    let g = |o, n| bits::get(&b, o, n).unwrap_or(0);
    let link = |h| Link::Standard(TC_STANDARD, h);
    let kind = g(0, 1);
    let cop = g(6, 2);
    let vcid = g(8, 6);
    let flags = [(16, "no RF available"), (17, "no bit lock"), (18, "lockout"), (19, "wait"), (20, "retransmit")];
    let mut children = vec![
        Field::new("control word type", at, 1, kind.to_string(), if kind == 0 { "CLCW" } else { "not a CLCW" }),
        Field::new("status", at + 3, 3, g(3, 3).to_string(), ""),
        Field::new("COP in effect", at + 6, 2, cop.to_string(), if cop == 1 { "COP-1" } else { "" }),
        Field::new("VCID", at + 8, 6, vcid.to_string(), ""),
    ];
    let mut up = Vec::new();
    for (bit, name) in flags {
        let v = g(bit, 1);
        let field = Field::new(name, at + bit, 1, v.to_string(), if v == 1 { "yes" } else { "no" });
        children.push(if v == 1 && matches!(name, "lockout" | "no RF available" | "no bit lock") { field.checked(Check::Warn(name.into())) } else { field });
        if v == 1 {
            up.push(name);
        }
    }
    children.push(Field::new("FARM-B counter", at + 21, 2, g(21, 2).to_string(), ""));
    let nr = g(24, 8);
    children.push(Field::new("report value N(R)", at + 24, 8, format!("0x{nr:02X}"), format!("next expected TC frame {nr}")));
    let mut f = Field::group("OCF · CLCW", children);
    f.raw = hex(&b);
    f.value = if up.is_empty() { "accepting".into() } else { up.join(", ") };
    f.link = Some(link("Communications Link Control Word"));
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tm_frame(fhp: u16, vc_count: u8) -> Vec<u8> {
        let mut f = Vec::new();
        bits::put(&mut f, 0, 2, 0);
        bits::put(&mut f, 2, 10, 0x0AB);
        bits::put(&mut f, 12, 3, 1);
        bits::put(&mut f, 15, 1, 1);
        bits::put(&mut f, 16, 8, 0x5C);
        bits::put(&mut f, 24, 8, vc_count as u64);
        bits::put(&mut f, 32, 16, fhp as u64);
        f.extend(std::iter::repeat_n(0xAA, 20));
        f.extend([0x01, 0x00, 0x00, 0x07]);
        f
    }

    #[test]
    fn a_tm_frame_and_its_clcw() {
        let p = FrameProfile { length: 30, fecf: false, vc_names: vec![(1, "HK".into())], ..Default::default() };
        let (f, info) = decode(&tm_frame(0x01A, 0x12), &p).unwrap();
        assert_eq!(&tm_frame(0x01A, 0x12)[..2], &[0x0A, 0xB3]);
        assert_eq!((info.scid, info.vcid, info.vc_count, info.first_header, info.ocf), (0xAB, 1, 0x12, Some(0x1A), Some(0x0100_0007)));
        assert_eq!(info.data, 6..26);
        assert_eq!(f.find("OCF · CLCW").unwrap().value, "accepting");
        assert_eq!(f.find("report value N(R)").unwrap().value, "next expected TC frame 7");
        assert!(f.name.contains("VC 1 (HK)"));
        let lock = clcw_field(0x0100_2007, 0);
        assert!(lock.value.contains("lockout"), "{}", lock.value);
    }

    #[test]
    fn a_tc_frame_with_fecf() {
        let mut f = Vec::new();
        bits::put(&mut f, 2, 1, 0);
        bits::put(&mut f, 6, 10, 0xAB);
        bits::put(&mut f, 16, 6, 0);
        bits::put(&mut f, 22, 10, 11);
        bits::put(&mut f, 32, 8, 7);
        f.push(0xC0);
        f.extend([1, 2, 3, 4]);
        let crc = crc::ccitt16(&f);
        f.extend(crc.to_be_bytes());
        let (field, info) = decode(&f, &FrameProfile { kind: FrameKind::Tc, fecf: true, ..Default::default() }).unwrap();
        assert!(!field.any_bad(), "{field:#?}");
        assert_eq!((info.vc_count, info.data), (7, 6..10));
        f[7] ^= 1;
        assert!(decode(&f, &FrameProfile { kind: FrameKind::Tc, fecf: true, ..Default::default() }).unwrap().0.any_bad());
    }
}
