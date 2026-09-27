//! A recording or a live feed, split into packets: back-to-back space
//! packets, records with a fixed header each, or frames -- found by their
//! sync marker, de-randomized, Reed-Solomon corrected -- with the packets
//! inside them reassembled across frame boundaries by virtual channel.
//! `Splitter` takes bytes in chunks, so a file and a socket read the same.

use std::collections::HashMap;

use crate::coding::{self, ASM};
use crate::frames::{self, FrameInfo, FrameKind, FrameProfile};
use crate::packet::PrimaryHeader;
use crate::rs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Framing {
    /// Space packets back to back.
    Packets,
    /// Each packet after a fixed-size record header.
    Records { header: usize },
    /// Transfer frames, with or without sync markers, as the profile says.
    Frames(FrameProfile),
}

impl Framing {
    pub fn label(&self) -> String {
        match self {
            Framing::Packets => "space packets".into(),
            Framing::Records { header } => format!("packets after {header}-octet records"),
            Framing::Frames(p) => {
                let kind = match p.kind {
                    FrameKind::Tm => "TM",
                    FrameKind::Tc => "TC",
                    FrameKind::Aos => "AOS",
                    FrameKind::Uslp => "USLP",
                };
                let mut s = format!("{kind} frames of {}", p.length);
                if p.asm {
                    s = format!("CADUs · {s}");
                }
                if p.rs_depth > 0 {
                    s.push_str(&format!(" · R-S I={}", p.rs_depth));
                }
                if p.randomized {
                    s.push_str(" · randomized");
                }
                s
            }
        }
    }
}

/// What the splitter found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Packet {
        /// Where it (or the frame it started in) begins in the input.
        offset: usize,
        bytes: Vec<u8>,
        vc: Option<u8>,
    },
    Frame {
        offset: usize,
        info: FrameInfo,
        /// The corrected (and de-randomized) frame.
        bytes: Vec<u8>,
        /// Frames lost before this one on its virtual channel.
        lost: u64,
        /// Symbols corrected, or `None` when a code word was beyond repair.
        rs: Option<Option<usize>>,
    },
    /// Bytes that made no sense, skipped.
    Junk { offset: usize, len: usize, why: String },
}

/// Code block length and virtual fill for a frame length and depth.
pub fn rs_geometry(frame_len: usize, depth: usize) -> Option<(usize, usize)> {
    if depth == 0 || !frame_len.is_multiple_of(depth) || frame_len / depth > 223 {
        return None;
    }
    let pad = 223 - frame_len / depth;
    Some((frame_len + 32 * depth, pad))
}

pub struct Splitter {
    framing: Framing,
    buf: Vec<u8>,
    /// Input offset of `buf[0]`.
    base: usize,
    partial: HashMap<u8, (usize, Vec<u8>)>,
    last_count: HashMap<u8, u64>,
    junk: Option<(usize, usize)>,
}

impl Splitter {
    pub fn new(framing: Framing) -> Splitter {
        Splitter { framing, buf: Vec::new(), base: 0, partial: HashMap::new(), last_count: HashMap::new(), junk: None }
    }

    pub fn framing(&self) -> &Framing {
        &self.framing
    }

    fn skip(&mut self, n: usize, out: &mut Vec<Item>, why: &str) {
        match &mut self.junk {
            Some((_, len)) => *len += n,
            None => self.junk = Some((self.base, n)),
        }
        let _ = (out, why);
        self.buf.drain(..n);
        self.base += n;
    }

    fn flush_junk(&mut self, out: &mut Vec<Item>, why: &str) {
        if let Some((offset, len)) = self.junk.take() {
            out.push(Item::Junk { offset, len, why: why.into() });
        }
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        let v: Vec<u8> = self.buf.drain(..n).collect();
        self.base += n;
        v
    }

    /// Feeds the next chunk of input; returns what it completed.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Item> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        match self.framing.clone() {
            Framing::Packets => self.packets(&mut out, 0),
            Framing::Records { header } => self.packets(&mut out, header),
            Framing::Frames(p) => self.frames(&p, &mut out),
        }
        out
    }

    /// What's left at the end of the input.
    pub fn finish(&mut self) -> Vec<Item> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let n = self.buf.len();
            self.skip(n, &mut out, "");
        }
        self.flush_junk(&mut out, "incomplete at the end");
        out
    }

    fn packets(&mut self, out: &mut Vec<Item>, record: usize) {
        loop {
            if self.buf.len() < record + 6 {
                return;
            }
            let Some(h) = PrimaryHeader::decode(&self.buf[record..]) else { return };
            if !h.plausible() {
                self.skip(1, out, "");
                continue;
            }
            let len = h.packet_len();
            if self.buf.len() < record + len {
                return;
            }
            self.flush_junk(out, "not a packet header");
            let offset = self.base + record;
            let mut all = self.take(record + len);
            let bytes = all.split_off(record);
            out.push(Item::Packet { offset, bytes, vc: None });
        }
    }

    fn frames(&mut self, p: &FrameProfile, out: &mut Vec<Item>) {
        let geometry = if p.rs_depth > 0 { rs_geometry(p.length, p.rs_depth) } else { None };
        let block = geometry.map(|(n, _)| n).unwrap_or(p.length);
        loop {
            let need = block + if p.asm { 4 } else { 0 };
            if p.asm {
                match self.buf.windows(4).position(|w| w == ASM) {
                    Some(0) => {}
                    Some(i) => {
                        self.skip(i, out, "");
                        continue;
                    }
                    None => {
                        if self.buf.len() > 3 {
                            let n = self.buf.len() - 3;
                            self.skip(n, out, "");
                        }
                        return;
                    }
                }
            }
            if self.buf.len() < need {
                return;
            }
            self.flush_junk(out, "no sync marker");
            let offset = self.base;
            let mut cadu = self.take(need);
            let mut frame = if p.asm { cadu.split_off(4) } else { cadu };
            if p.randomized {
                coding::derandomize(&mut frame);
            }
            let mut rs = None;
            if let Some((_, pad)) = geometry {
                let outcomes = rs::decode_block(&mut frame, p.rs_depth, pad);
                rs = Some(if outcomes.contains(&rs::Outcome::Uncorrectable) {
                    None
                } else {
                    Some(outcomes.iter().map(|o| if let rs::Outcome::Corrected(v) = o { v.len() } else { 0 }).sum())
                });
                frame.truncate(p.length);
            }
            let Some((_, info)) = frames::decode(&frame, p) else {
                out.push(Item::Junk { offset, len: need, why: "not a frame".into() });
                continue;
            };
            let modulus = match p.kind {
                FrameKind::Aos => 1u64 << 24,
                _ => 256,
            };
            let lost = match self.last_count.insert(info.vcid, info.vc_count) {
                Some(last) => (info.vc_count + modulus - last - 1) % modulus,
                None => 0,
            };
            if lost > 0 {
                self.partial.remove(&info.vcid);
            }
            let data = frame.get(info.data.clone()).unwrap_or(&[]).to_vec();
            let vc = info.vcid;
            out.push(Item::Frame { offset, info: info.clone(), bytes: frame, lost, rs });
            if p.kind == FrameKind::Tc {
                continue;
            }
            match info.first_header {
                None => {
                    if let Some((_, part)) = self.partial.get_mut(&vc) {
                        part.extend_from_slice(&data);
                    }
                }
                Some(u16::MAX) => {
                    self.partial.remove(&vc);
                }
                Some(n) => {
                    let n = (n as usize).min(data.len());
                    if let Some((start, mut part)) = self.partial.remove(&vc) {
                        part.extend_from_slice(&data[..n]);
                        drain_packets(&mut part, start, vc, out);
                    }
                    self.partial.insert(vc, (offset, data[n..].to_vec()));
                }
            }
            if let Some((start, part)) = self.partial.get_mut(&vc) {
                let s = *start;
                drain_packets(part, s, vc, out);
                *start = offset;
            }
        }
    }
}

/// Complete packets off the front of `part`.
fn drain_packets(part: &mut Vec<u8>, offset: usize, vc: u8, out: &mut Vec<Item>) {
    loop {
        let Some(h) = PrimaryHeader::decode(part) else { return };
        if !h.plausible() {
            part.clear();
            return;
        }
        let len = h.packet_len();
        if part.len() < len {
            return;
        }
        let bytes: Vec<u8> = part.drain(..len).collect();
        out.push(Item::Packet { offset, bytes, vc: Some(vc) });
    }
}

/// Every item in `bytes`, split as `framing` says.
pub fn split(bytes: &[u8], framing: Framing) -> Vec<Item> {
    let mut s = Splitter::new(framing);
    let mut out = s.feed(bytes);
    out.extend(s.finish());
    out
}

/// A good guess at how `bytes` (the start of a recording) is framed, and
/// why.
pub fn guess(bytes: &[u8]) -> (Framing, String) {
    let asm = coding::find_asm(bytes);
    if asm.len() >= 3 {
        let stride = asm[1] - asm[0];
        if asm.windows(2).take(8).all(|w| w[1] - w[0] == stride) && stride > 10 {
            let block = stride - 4;
            let depth = (1..=8).find(|d| block == 255 * d);
            let length = depth.map(|d| 223 * d).unwrap_or(block);
            let head = &bytes[asm[0] + 4..];
            let mut derand = head[..head.len().min(8)].to_vec();
            coding::derandomize(&mut derand);
            let version = |b: &[u8]| b.first().map(|x| x >> 6).unwrap_or(9);
            let (randomized, first) = if version(head) != 0 && version(&derand) == 0 { (true, derand) } else { (false, head[..head.len().min(8)].to_vec()) };
            let kind = match first.first().map(|x| x >> 4) {
                Some(0b1100) => FrameKind::Uslp,
                Some(v) if v >> 2 == 1 => FrameKind::Aos,
                _ => FrameKind::Tm,
            };
            let ocf = kind == FrameKind::Tm && first.get(1).is_some_and(|b| b & 1 == 1);
            let p = FrameProfile { kind, length, asm: true, randomized, rs_depth: depth.unwrap_or(0), ocf, fecf: false, ..Default::default() };
            let why = format!("a sync marker every {stride} octets");
            return (Framing::Frames(p), why);
        }
    }
    let run = |start: usize, record: usize| {
        let mut at = start;
        let mut n = 0;
        while n < 4 {
            let Some(h) = bytes.get(at + record..).and_then(PrimaryHeader::decode) else { break };
            if !h.plausible() || at + record + h.packet_len() > bytes.len() {
                break;
            }
            at += record + h.packet_len();
            n += 1;
        }
        n
    };
    if run(0, 0) >= 3 || (run(0, 0) >= 1 && bytes.len() < 4096) {
        return (Framing::Packets, "packet headers chain from the first octet".into());
    }
    for header in 1..=64 {
        if run(0, header) >= 3 {
            return (Framing::Records { header }, format!("a packet after every {header}-octet record header"));
        }
    }
    (Framing::Packets, "no framing recognized -- reading as packets".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;
    use crate::rs;

    fn pkt(apid: u16, seq: u16, n: usize) -> Vec<u8> {
        Packet::build(PrimaryHeader { secondary_header: false, apid, seq_flags: 3, seq_count: seq, ..Default::default() }, &vec![seq as u8; n]).bytes
    }

    /// The packets found, idle ones left out.
    fn packets_of(items: &[Item]) -> Vec<Vec<u8>> {
        items
            .iter()
            .filter_map(|i| if let Item::Packet { bytes, .. } = i { Some(bytes.clone()) } else { None })
            .filter(|b| PrimaryHeader::decode(b).is_some_and(|h| h.apid != crate::packet::IDLE_APID))
            .collect()
    }

    #[test]
    fn back_to_back_packets_in_any_chunks_and_junk_between() {
        let mut all = Vec::new();
        for s in 0..5 {
            all.extend(pkt(0x10, s, 10 + s as usize));
        }
        all.extend([0xE0, 0xE0, 0xE0]);
        all.extend(pkt(0x10, 5, 4));
        assert_eq!(guess(&all).0, Framing::Packets);
        let mut s = Splitter::new(Framing::Packets);
        let mut items = Vec::new();
        for chunk in all.chunks(7) {
            items.extend(s.feed(chunk));
        }
        items.extend(s.finish());
        assert_eq!(packets_of(&items).len(), 6);
        assert!(items.iter().any(|i| matches!(i, Item::Junk { len: 3, .. })));
    }

    #[test]
    fn records_with_a_header_are_guessed() {
        let mut all = Vec::new();
        for s in 0..4 {
            all.extend([0xAA; 12]);
            all.extend(pkt(0x20, s, 8));
        }
        assert_eq!(guess(&all).0, Framing::Records { header: 12 });
        assert_eq!(packets_of(&split(&all, Framing::Records { header: 12 })).len(), 4);
    }

    /// TM frames of `len` carrying `stream`, then an idle packet as fill,
    /// with the first header pointer set right.
    fn frames_for(stream: &[Vec<u8>], len: usize) -> Vec<Vec<u8>> {
        let data_len = len - 6 - 4;
        let mut flat = Vec::new();
        let mut starts = Vec::new();
        for p in stream {
            starts.push(flat.len());
            flat.extend(p);
        }
        let fill = data_len - flat.len() % data_len;
        flat.extend(Packet::build(PrimaryHeader { apid: crate::packet::IDLE_APID, seq_flags: 3, ..Default::default() }, &vec![0; fill.max(8) + data_len]).bytes);
        flat.truncate(flat.len() - flat.len() % data_len);
        let mut out = Vec::new();
        let mut count: u8 = 0;
        for (i, chunk) in flat.chunks(data_len).enumerate() {
            let base = i * data_len;
            let fhp = starts.iter().find(|&&s| s >= base && s < base + data_len).map(|s| s - base).unwrap_or(0x7FF);
            let mut f = Vec::new();
            crate::bits::put(&mut f, 2, 10, 0xAB);
            crate::bits::put(&mut f, 15, 1, 1);
            crate::bits::put(&mut f, 24, 8, count as u64);
            crate::bits::put(&mut f, 32, 16, fhp as u64);
            let mut c = chunk.to_vec();
            c.resize(data_len, 0);
            f.extend(c);
            f.extend([0x01, 0, 0, 0]);
            out.push(f);
            count = count.wrapping_add(1);
        }
        out
    }

    #[test]
    fn cadus_are_derandomized_corrected_and_packets_reassembled() {
        let stream: Vec<Vec<u8>> = (0..12).map(|s| pkt(0x3F2, s, 40 + s as usize * 7)).collect();
        let depth = 1;
        let len = 223;
        let mut file = Vec::new();
        for (i, mut f) in frames_for(&stream, len).into_iter().enumerate() {
            f.extend(rs::encode_block(&f, depth, 0));
            if i == 1 {
                f[10] ^= 0xFF; // an error R-S corrects
            }
            coding::derandomize(&mut f);
            file.extend(ASM);
            file.extend(f);
        }
        let (framing, _) = guess(&file);
        let Framing::Frames(p) = &framing else { panic!("{framing:?}") };
        assert_eq!((p.length, p.rs_depth, p.randomized, p.ocf), (223, 1, true, true));
        let items = split(&file, framing);
        assert_eq!(packets_of(&items), stream[..packets_of(&items).len()].to_vec());
        assert!(packets_of(&items).len() >= 10, "{}", packets_of(&items).len());
        assert!(items.iter().any(|i| matches!(i, Item::Frame { rs: Some(Some(1)), .. })));
    }

    #[test]
    fn a_lost_frame_is_counted_and_its_partial_packet_dropped() {
        let stream: Vec<Vec<u8>> = (0..6).map(|s| pkt(0x3F2, s, 50)).collect();
        let frames = frames_for(&stream, 60);
        let p = FrameProfile { length: 60, asm: false, ocf: true, ..Default::default() };
        let mut file = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            if i != 2 {
                file.extend(f);
            }
        }
        let items = split(&file, Framing::Frames(p));
        assert!(items.iter().any(|i| matches!(i, Item::Frame { lost: 1, .. })), "{items:?}");
        let got = packets_of(&items);
        assert!(got.iter().all(|g| stream.contains(g)), "only whole packets");
        assert!(got.len() < stream.len());
    }
}
