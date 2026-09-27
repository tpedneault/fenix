//! The space packet (CCSDS 133.0-B): a 6-octet primary header, then the
//! data field -- a secondary header when the flag says so, then user data.

use crate::bits;
use crate::field::{hex, Check, Field, Link};

pub const STANDARD: &str = "CCSDS 133.0-B";
/// Idle packets carry this APID.
pub const IDLE_APID: u16 = 0x7FF;
/// The longest packet: the header and 65,536 octets of data field.
pub const MAX_LEN: usize = 6 + 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PrimaryHeader {
    pub version: u8,
    /// A telecommand (packet type 1).
    pub is_tc: bool,
    pub secondary_header: bool,
    pub apid: u16,
    /// 0 continuation, 1 first, 2 last, 3 unsegmented.
    pub seq_flags: u8,
    pub seq_count: u16,
    /// The data field's length minus one.
    pub data_length: u16,
}

impl PrimaryHeader {
    pub fn decode(bytes: &[u8]) -> Option<PrimaryHeader> {
        if bytes.len() < 6 {
            return None;
        }
        let g = |at, n| bits::get(bytes, at, n).unwrap_or(0);
        Some(PrimaryHeader {
            version: g(0, 3) as u8,
            is_tc: g(3, 1) == 1,
            secondary_header: g(4, 1) == 1,
            apid: g(5, 11) as u16,
            seq_flags: g(16, 2) as u8,
            seq_count: g(18, 14) as u16,
            data_length: g(32, 16) as u16,
        })
    }

    pub fn encode(&self) -> [u8; 6] {
        let mut out = Vec::with_capacity(6);
        bits::put(&mut out, 0, 3, self.version as u64);
        bits::put(&mut out, 3, 1, self.is_tc as u64);
        bits::put(&mut out, 4, 1, self.secondary_header as u64);
        bits::put(&mut out, 5, 11, self.apid as u64);
        bits::put(&mut out, 16, 2, self.seq_flags as u64);
        bits::put(&mut out, 18, 14, self.seq_count as u64);
        bits::put(&mut out, 32, 16, self.data_length as u64);
        out.try_into().unwrap_or([0; 6])
    }

    /// The whole packet's length, header included.
    pub fn packet_len(&self) -> usize {
        self.data_length as usize + 7
    }

    /// Whether this looks like a real header: version 0.
    pub fn plausible(&self) -> bool {
        self.version == 0
    }
}

pub fn seq_flags_name(f: u8) -> &'static str {
    match f {
        0 => "continuation",
        1 => "first segment",
        2 => "last segment",
        _ => "unsegmented",
    }
}

/// A space packet: its header, and all its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub header: PrimaryHeader,
    pub bytes: Vec<u8>,
}

impl Packet {
    /// The packet at the start of `bytes`: exactly as long as its header
    /// says when there are enough bytes, else what's there (and `short`).
    pub fn parse(bytes: &[u8]) -> Option<(Packet, bool)> {
        let header = PrimaryHeader::decode(bytes)?;
        let len = header.packet_len();
        let short = bytes.len() < len;
        let take = len.min(bytes.len());
        Some((Packet { header, bytes: bytes[..take].to_vec() }, short))
    }

    /// The data field: everything after the primary header.
    pub fn data(&self) -> &[u8] {
        self.bytes.get(6..).unwrap_or(&[])
    }

    /// A packet from a header and a data field, the length filled in.
    pub fn build(mut header: PrimaryHeader, data: &[u8]) -> Packet {
        header.data_length = data.len().saturating_sub(1) as u16;
        let mut bytes = header.encode().to_vec();
        bytes.extend_from_slice(data);
        Packet { header, bytes }
    }
}

/// The primary header as fields, with its checks: version 0, the length
/// against the bytes present, the idle APID.
pub fn header_field(bytes: &[u8]) -> Option<Field> {
    let h = PrimaryHeader::decode(bytes)?;
    let std = |heading| Link::Standard(STANDARD, heading);
    let version = Field::new("version", 0, 3, h.version.to_string(), if h.version == 0 { "space packet" } else { "not a space packet" })
        .checked(if h.version == 0 { Check::Ok("version 0".into()) } else { Check::Bad(format!("version {} -- 133.0 packets are version 0", h.version)) })
        .linked(std("Packet Version Number"));
    let kind = Field::new("type", 3, 1, (h.is_tc as u8).to_string(), if h.is_tc { "telecommand" } else { "telemetry" }).linked(std("Packet Type"));
    let sec = Field::new("secondary header", 4, 1, (h.secondary_header as u8).to_string(), if h.secondary_header { "present" } else { "absent" }).linked(std("Secondary Header Flag"));
    let mut apid = Field::new("APID", 5, 11, format!("0x{:03X}", h.apid), h.apid.to_string()).linked(std("Application Process Identifier"));
    if h.apid == IDLE_APID {
        apid = apid.with_value("2047 -- idle packet");
    }
    let flags = Field::new("sequence flags", 16, 2, format!("{:02b}", h.seq_flags), seq_flags_name(h.seq_flags)).linked(std("Sequence Flags"));
    let count = Field::new(if h.is_tc { "sequence count or name" } else { "sequence count" }, 18, 14, format!("0x{:04X}", h.seq_count), h.seq_count.to_string())
        .linked(std("Packet Sequence Count or Packet Name"));
    let want = h.packet_len();
    let length_check = match bytes.len().cmp(&want) {
        std::cmp::Ordering::Equal => Check::Ok(format!("{want} octets")),
        std::cmp::Ordering::Less => Check::Bad(format!("says {want} octets, {} here", bytes.len())),
        std::cmp::Ordering::Greater => Check::Warn(format!("{want} octets, {} more follow", bytes.len() - want)),
    };
    let length = Field::new("data length", 32, 16, format!("0x{:04X}", h.data_length), format!("{} octets of data", h.data_length as usize + 1))
        .checked(length_check)
        .linked(std("Packet Data Length"));
    let mut g = Field::group("Primary header", vec![version, kind, sec, apid, flags, count, length]);
    g.raw = hex(&bytes[..6]);
    Some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TM: [u8; 29] = [
        0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16, 0x20, 0x03, 0x19, 0x00, 0x42, 0x00, 0x00, 0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00, 0x00, 0x01, 0x01, 0x0B, 0xB8, 0x0A, 0x28,
        0x02, 0x60, 0xE5,
    ];

    #[test]
    fn the_example_header_decodes_and_encodes_back() {
        let h = PrimaryHeader::decode(&TM).unwrap();
        assert_eq!((h.is_tc, h.secondary_header, h.apid, h.seq_flags, h.seq_count, h.packet_len()), (false, true, 0x3F2, 3, 0x123, 29));
        assert_eq!(h.encode(), [0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16]);
        let f = header_field(&TM).unwrap();
        assert!(!f.any_bad());
        assert_eq!(f.find("APID").unwrap().value, "1010");
        assert!(header_field(&TM[..20]).unwrap().any_bad(), "short");
    }

    #[test]
    fn a_built_packet_carries_its_length() {
        let h = PrimaryHeader { is_tc: true, secondary_header: true, apid: 0x3F2, seq_flags: 3, seq_count: 7, ..Default::default() };
        let p = Packet::build(h, &[1, 2, 3]);
        assert_eq!(p.bytes, vec![0x1B, 0xF2, 0xC0, 0x07, 0x00, 0x02, 1, 2, 3]);
        let (back, short) = Packet::parse(&p.bytes).unwrap();
        assert!(!short);
        assert_eq!(back.data(), &[1, 2, 3]);
    }
}
