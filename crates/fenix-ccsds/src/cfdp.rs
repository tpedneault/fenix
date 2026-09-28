//! CFDP (727.0-B) PDUs: enough to read a file transfer's progress out of
//! a recording -- the header, which directive, and a file data PDU's
//! offset.

use crate::bits;
use crate::crc;
use crate::field::{hex, Check, Field, Link};

pub const STANDARD: &str = "CCSDS 727.0-B";

pub fn directive_name(code: u8) -> &'static str {
    match code {
        0x04 => "EOF",
        0x05 => "Finished",
        0x06 => "ACK",
        0x07 => "Metadata",
        0x08 => "NAK",
        0x09 => "Prompt",
        0x0C => "Keep Alive",
        _ => "unknown directive",
    }
}

/// A PDU's fields.
pub fn decode(b: &[u8]) -> Option<Field> {
    let g = |at, n| bits::get(b, at, n);
    let link = |h| Link::Standard(STANDARD, h);
    let version = g(0, 3)?;
    let file_data = g(3, 1)? == 1;
    let toward_sender = g(4, 1)? == 1;
    let unack = g(5, 1)? == 1;
    let crc_flag = g(6, 1)? == 1;
    let large = g(7, 1)? == 1;
    let data_len = g(8, 16)? as usize;
    let seg_ctrl = g(24, 1)?;
    let eid_len = g(25, 3)? as usize + 1;
    let seg_meta = g(28, 1)?;
    let seq_len = g(29, 3)? as usize + 1;
    let mut header = vec![
        Field::new("version", 0, 3, version.to_string(), if version == 1 { "CFDP version 2" } else { "" }),
        Field::new("PDU type", 3, 1, (file_data as u8).to_string(), if file_data { "file data" } else { "file directive" }).linked(link("PDU Type")),
        Field::new("direction", 4, 1, (toward_sender as u8).to_string(), if toward_sender { "toward sender" } else { "toward receiver" }),
        Field::new("transmission mode", 5, 1, (unack as u8).to_string(), if unack { "unacknowledged" } else { "acknowledged" }),
        Field::new("CRC flag", 6, 1, (crc_flag as u8).to_string(), if crc_flag { "CRC present" } else { "no CRC" }),
        Field::new("large file", 7, 1, (large as u8).to_string(), if large { "64-bit sizes" } else { "32-bit sizes" }),
        Field::new("data field length", 8, 16, data_len.to_string(), format!("{data_len} octets")),
        Field::new("segmentation control", 24, 1, seg_ctrl.to_string(), ""),
        Field::new("segment metadata", 28, 1, seg_meta.to_string(), ""),
    ];
    let mut at = 4;
    let src = g(at * 8, eid_len * 8)?;
    header.push(Field::new("source entity", at * 8, eid_len * 8, src.to_string(), ""));
    at += eid_len;
    let seq = g(at * 8, seq_len * 8)?;
    header.push(Field::new("transaction sequence", at * 8, seq_len * 8, seq.to_string(), ""));
    at += seq_len;
    let dst = g(at * 8, eid_len * 8)?;
    header.push(Field::new("destination entity", at * 8, eid_len * 8, dst.to_string(), ""));
    at += eid_len;
    let mut children = vec![Field::group("PDU header", header)];
    let title = if file_data {
        let ol = if large { 8 } else { 4 };
        let offset = g(at * 8, ol * 8)?;
        let n = data_len.saturating_sub(ol + if crc_flag { 2 } else { 0 });
        children.push(Field::new("offset", at * 8, ol * 8, offset.to_string(), format!("{n} octets at {offset}")));
        format!("CFDP file data · transaction {seq}")
    } else {
        let code = *b.get(at)?;
        children.push(Field::new("directive", at * 8, 8, format!("0x{code:02X}"), directive_name(code)).linked(link("File Directive Codes")));
        let body = b.get(at + 1..(at + data_len).min(b.len())).unwrap_or(&[]);
        if !body.is_empty() {
            children.push(Field::new("parameters", (at + 1) * 8, body.len() * 8, hex(body), ""));
        }
        format!("CFDP {} · transaction {seq}", directive_name(code))
    };
    if crc_flag {
        let end = (at + data_len).min(b.len());
        if end >= 2 {
            let raw = u16::from_be_bytes([b[end - 2], b[end - 1]]);
            let want = crc::ccitt16(&b[..end - 2]);
            children.push(Field::new("CRC", (end - 2) * 8, 16, format!("{raw:04X}"), "").checked(if raw == want { Check::Ok("matches".into()) } else { Check::Bad(format!("computed {want:04X}")) }));
        }
    }
    Some(Field::group(title, children))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_eof_directive_reads() {
        // version 1, directive, toward receiver, unacknowledged, no CRC;
        // 1-octet entity IDs and sequence number.
        let pdu = [0x24, 0x00, 0x0A, 0x00, 0x01, 0x2A, 0x02, 0x04, 0x00, 0, 0, 0, 0, 0, 0, 0, 0];
        let f = decode(&pdu).unwrap();
        assert_eq!(f.name, "CFDP EOF · transaction 42");
        assert_eq!(f.find("source entity").unwrap().raw, "1");
        assert_eq!(f.find("transmission mode").unwrap().value, "unacknowledged");
    }
}
