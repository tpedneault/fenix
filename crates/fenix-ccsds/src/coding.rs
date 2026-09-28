//! Synchronization and channel coding around frames: the attached sync
//! marker and pseudo-randomizer of 131.0-B (TM), and the CLTU with its
//! BCH(63,56) code blocks of 231.0-B (TC).

use crate::field::{hex, hex_tight, Check, Field, Link};

pub const TM_STANDARD: &str = "CCSDS 131.0-B";
pub const TC_STANDARD: &str = "CCSDS 231.0-B";
pub const ASM: [u8; 4] = [0x1A, 0xCF, 0xFC, 0x1D];
pub const CLTU_START: [u8; 2] = [0xEB, 0x90];
pub const CLTU_TAIL: [u8; 8] = [0xC5, 0xC5, 0xC5, 0xC5, 0xC5, 0xC5, 0xC5, 0x79];

/// Every offset `ASM` starts at.
pub fn find_asm(bytes: &[u8]) -> Vec<usize> {
    bytes.windows(4).enumerate().filter(|(_, w)| *w == ASM).map(|(i, _)| i).collect()
}

/// The pseudo-random sequence, `h(x) = x^8+x^7+x^5+x^3+1` from all ones:
/// `n` octets of it (it repeats every 255).
pub fn randomizer(n: usize) -> Vec<u8> {
    let mut bits = vec![1u8; 8];
    while bits.len() < n * 8 {
        let k = bits.len() - 8;
        bits.push(bits[k + 7] ^ bits[k + 5] ^ bits[k + 3] ^ bits[k]);
    }
    bits.chunks(8).take(n).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | b)).collect()
}

/// XORs the randomizer over `bytes` (it's its own inverse).
pub fn derandomize(bytes: &mut [u8]) {
    let seq = randomizer(bytes.len());
    for (b, r) in bytes.iter_mut().zip(seq) {
        *b ^= r;
    }
}

/// The parity octet of a BCH(63,56) code block for 7 information octets:
/// 7 parity bits (the remainder by `g(x) = x^7+x^6+x^2+1`, complemented),
/// then the filler bit 0.
pub fn bch_parity(info: &[u8; 7]) -> u8 {
    let mut reg: u8 = 0;
    for &byte in info {
        for k in (0..8).rev() {
            let bit = (byte >> k) & 1;
            let feedback = ((reg >> 6) & 1) ^ bit;
            reg = (reg << 1) & 0x7F;
            if feedback == 1 {
                reg ^= 0b100_0101; // x^6 + x^2 + 1: g(x) without its x^7
            }
        }
    }
    (!reg & 0x7F) << 1
}

/// A CLTU for `frame`: the start sequence, code blocks (the last padded
/// with 0x55), the tail sequence.
pub fn cltu_encode(frame: &[u8]) -> Vec<u8> {
    let mut out = CLTU_START.to_vec();
    for chunk in frame.chunks(7) {
        let mut info = [0x55u8; 7];
        info[..chunk.len()].copy_from_slice(chunk);
        out.extend_from_slice(&info);
        out.push(bch_parity(&info));
    }
    out.extend_from_slice(&CLTU_TAIL);
    out
}

/// How a code block reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// Its parity matches.
    Good([u8; 7]),
    /// One bit was wrong and is put right: the information, and which bit
    /// (0 the first sent, 56-62 the parity's).
    Corrected([u8; 7], usize),
    /// More wrong than the decoder's mode handles.
    Bad,
}

/// Checks a code block the way a spacecraft's CLTU decoder does (231.0-B):
/// the filler bit isn't looked at, and in error-correcting mode
/// (`correct`) a single wrong bit is put right. BCH(63,56) is at distance
/// 4, so one wrong bit has one fix and two have none.
pub fn bch_check(block: &[u8], correct: bool) -> Block {
    let Some(info) = block.get(..7).and_then(|b| <[u8; 7]>::try_from(b).ok()) else { return Block::Bad };
    let Some(&parity) = block.get(7) else { return Block::Bad };
    let fits = |info: &[u8; 7], parity: u8| bch_parity(info) & 0xFE == parity & 0xFE;
    if fits(&info, parity) {
        return Block::Good(info);
    }
    if !correct {
        return Block::Bad;
    }
    for bit in 0..63 {
        let (mut i, mut p) = (info, parity);
        if bit < 56 {
            i[bit / 8] ^= 0x80 >> (bit % 8);
        } else {
            p ^= 0x80 >> (bit - 56);
        }
        if fits(&i, p) {
            return Block::Corrected(i, bit);
        }
    }
    Block::Bad
}

/// A CLTU's layers, and the frame bytes it carries (fill included --
/// the frame's own length says where it ends). `correct`: the decoder's
/// error-correcting mode.
pub fn cltu_decode(bytes: &[u8], correct: bool) -> Option<(Field, Vec<u8>)> {
    let link = |h| Link::Standard(TC_STANDARD, h);
    let start = bytes.windows(2).position(|w| w == CLTU_START)?;
    let mut children = vec![Field::new("start sequence", start * 8, 16, "EB90", "").checked(Check::Ok("found".into())).linked(link("Start Sequence"))];
    let mut data = Vec::new();
    let mut at = start + 2;
    let (mut bad, mut fixed) = (0, 0);
    let mut blocks = Vec::new();
    while at + 8 <= bytes.len() {
        let block = &bytes[at..at + 8];
        if block == CLTU_TAIL {
            break;
        }
        let name = format!("code block {}", blocks.len() + 1);
        let (info, field) = match bch_check(block, correct) {
            Block::Good(info) => (info, Field::new(name, at * 8, 64, hex(block), "").checked(Check::Ok("parity ok".into()))),
            Block::Corrected(info, bit) => {
                fixed += 1;
                let what = if bit < 56 { format!("octet {}, bit {} was wrong", bit / 8 + 1, bit % 8) } else { format!("parity bit {} was wrong", bit - 56) };
                (info, Field::new(name, at * 8, 64, hex(block), what).checked(Check::Warn("1 bit corrected".into())))
            }
            Block::Bad => {
                bad += 1;
                let info: [u8; 7] = block[..7].try_into().ok()?;
                let want = bch_parity(&info);
                let more = if correct { " -- more than one bit wrong" } else { "" };
                (info, Field::new(name, at * 8, 64, hex(block), "").checked(Check::Bad(format!("parity {:02X}, expected {want:02X}{more}", block[7]))))
            }
        };
        blocks.push(field);
        data.extend_from_slice(&info);
        at += 8;
    }
    let n = blocks.len();
    let mut cb = Field::group(format!("{n} code blocks"), blocks);
    cb.value = if correct { "error-correcting mode".into() } else { "error-detecting mode".into() };
    cb.check = Some(match (bad, fixed) {
        (0, 0) => Check::Ok("all parities match".into()),
        (0, f) => Check::Warn(format!("{f} corrected")),
        (b, _) => Check::Bad(format!("{b} bad")),
    });
    cb.link = Some(link("BCH Codeblock"));
    children.push(cb);
    if bytes.get(at..at + 8) == Some(&CLTU_TAIL[..]) {
        children.push(Field::new("tail sequence", at * 8, 64, hex_tight(&CLTU_TAIL), "").checked(Check::Ok("found".into())).linked(link("Tail Sequence")));
    } else {
        children.push(Field::new("tail sequence", at * 8, 0, "", "").checked(Check::Warn("not found".into())));
    }
    Some((Field::group("CLTU", children), data))
}

/// Where data octet `d` of the CLTU whose start sequence is at `start`
/// sits: seven data octets to a code block, then the block's parity.
pub fn cltu_octet(start: usize, d: usize) -> usize {
    start + 2 + d / 7 * 8 + d % 7
}

/// What ended a CLTU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CltuEnd {
    /// The tail sequence.
    Tail,
    /// Code block `n` (from 1) failed its parity; it wasn't the tail.
    BadBlock(usize),
}

/// A CLTU as the spacecraft's decoder reads it (231.0-B): from the start
/// sequence, block by block, until a block fails -- the tail sequence is
/// built to, in either mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CltuRead {
    /// Where the start sequence is.
    pub start: usize,
    /// Just after the block that ended it.
    pub end: usize,
    /// The information octets of the blocks before that one, corrected.
    pub data: Vec<u8>,
    pub ended: CltuEnd,
    /// The code blocks (from 1) that had a bit put right.
    pub corrected: Vec<usize>,
}

/// The first CLTU in `bytes`, read in error-correcting mode when
/// `correct`; `None` when there's no start sequence, or the input ends
/// before something ends the CLTU.
pub fn cltu_read(bytes: &[u8], correct: bool) -> Option<CltuRead> {
    let start = bytes.windows(2).position(|w| w == CLTU_START)?;
    let mut at = start + 2;
    let mut data = Vec::new();
    let mut corrected = Vec::new();
    loop {
        let block = bytes.get(at..at + 8)?;
        let n = (at - start - 2) / 8 + 1;
        match bch_check(block, correct) {
            Block::Good(info) => data.extend_from_slice(&info),
            Block::Corrected(info, _) => {
                data.extend_from_slice(&info);
                corrected.push(n);
            }
            Block::Bad => {
                let ended = if block == CLTU_TAIL { CltuEnd::Tail } else { CltuEnd::BadBlock(n) };
                return Some(CltuRead { start, end: at + 8, data, ended, corrected });
            }
        }
        at += 8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_randomizer_starts_as_the_standard_says() {
        assert_eq!(randomizer(4), vec![0xFF, 0x48, 0x0E, 0xC0]);
        let r = randomizer(510);
        assert_eq!(r[..255], r[255..], "period 255");
        let mut x = vec![1, 2, 3];
        derandomize(&mut x);
        derandomize(&mut x);
        assert_eq!(x, vec![1, 2, 3]);
        assert_eq!(find_asm(&[0, 0x1A, 0xCF, 0xFC, 0x1D, 9, 0x1A, 0xCF, 0xFC, 0x1D]), vec![1, 6]);
    }

    #[test]
    fn a_cltu_round_trips_and_a_flipped_bit_is_caught() {
        let frame: Vec<u8> = (0..16).collect();
        let cltu = cltu_encode(&frame);
        assert_eq!(cltu.len(), 2 + 3 * 8 + 8);
        assert_eq!(cltu[cltu.len() - 1], 0x79);
        let (f, data) = cltu_decode(&cltu, false).unwrap();
        assert!(!f.any_bad());
        assert_eq!(&data[..16], &frame[..]);
        let mut broken = cltu.clone();
        broken[5] ^= 0x04;
        assert!(cltu_decode(&broken, false).unwrap().0.any_bad());
        assert!(!cltu_decode(&broken, true).unwrap().0.any_bad(), "one bit: corrected");
        // Every parity octet ends in the filler bit 0.
        assert!(cltu[2..cltu.len() - 8].chunks(8).all(|b| b[7] & 1 == 0));
    }

    #[test]
    fn the_tail_fails_in_either_mode_and_every_single_bit_is_put_right() {
        assert_eq!(bch_check(&CLTU_TAIL, true), Block::Bad, "the tail is built never to decode");
        let info = [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE];
        let mut block = info.to_vec();
        block.push(bch_parity(&info));
        for bit in 0..63 {
            let mut b = block.clone();
            b[bit / 8] ^= 0x80 >> (bit % 8);
            assert_eq!(bch_check(&b, true), Block::Corrected(info, bit), "bit {bit}");
            assert_eq!(bch_check(&b, false), Block::Bad);
        }
        let mut filler = block.clone();
        filler[7] ^= 0x01;
        assert_eq!(bch_check(&filler, false), Block::Good(info), "the filler bit isn't looked at");
    }

    #[test]
    fn a_cltu_is_read_until_a_block_fails_as_the_tail_does() {
        let frame: Vec<u8> = (0..16).collect();
        let mut stream = vec![0x55, 0x55];
        stream.extend(cltu_encode(&frame));
        let r = cltu_read(&stream, false).unwrap();
        assert_eq!((r.start, r.end, r.ended), (2, stream.len(), CltuEnd::Tail));
        assert_eq!(&r.data[..16], &frame[..]);
        assert_eq!(stream[cltu_octet(2, 7)], 7, "the eighth octet opens the second block");
        let mut broken = stream.clone();
        broken[2 + 2 + 8 + 3] ^= 0x10;
        let r = cltu_read(&broken, false).unwrap();
        assert_eq!((r.ended, r.data.len()), (CltuEnd::BadBlock(2), 7), "the rest is abandoned");
        assert!(cltu_read(&stream[..stream.len() - 1], false).is_none(), "not ended yet");
        // In error-correcting mode the same flipped bit is put right.
        let r = cltu_read(&broken, true).unwrap();
        assert_eq!((r.ended, &r.data[..16], r.corrected.clone()), (CltuEnd::Tail, &frame[..], vec![2]));
        // Two bits in one block are beyond it: the CLTU ends there.
        let mut two = broken.clone();
        two[2 + 2 + 8 + 5] ^= 0x01;
        assert_eq!(cltu_read(&two, true).unwrap().ended, CltuEnd::BadBlock(2));
    }
}
