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

/// A CLTU's layers, and the frame bytes it carries (fill included --
/// the frame's own length says where it ends).
pub fn cltu_decode(bytes: &[u8]) -> Option<(Field, Vec<u8>)> {
    let link = |h| Link::Standard(TC_STANDARD, h);
    let start = bytes.windows(2).position(|w| w == CLTU_START)?;
    let mut children = vec![Field::new("start sequence", start * 8, 16, "EB90", "").checked(Check::Ok("found".into())).linked(link("Start Sequence"))];
    let mut data = Vec::new();
    let mut at = start + 2;
    let mut bad = 0;
    let mut blocks = Vec::new();
    while at + 8 <= bytes.len() {
        let block = &bytes[at..at + 8];
        if block == CLTU_TAIL {
            break;
        }
        let info: [u8; 7] = block[..7].try_into().ok()?;
        let want = bch_parity(&info);
        let ok = want == block[7];
        if !ok {
            bad += 1;
        }
        blocks.push(
            Field::new(format!("code block {}", blocks.len() + 1), at * 8, 64, hex(block), "")
                .checked(if ok { Check::Ok("parity ok".into()) } else { Check::Bad(format!("parity {:02X}, expected {want:02X}", block[7])) }),
        );
        data.extend_from_slice(&info);
        at += 8;
    }
    let n = blocks.len();
    let mut cb = Field::group(format!("{n} code blocks"), blocks);
    cb.check = Some(if bad == 0 { Check::Ok("all parities match".into()) } else { Check::Bad(format!("{bad} bad")) });
    cb.link = Some(link("BCH Codeblock"));
    children.push(cb);
    if bytes.get(at..at + 8) == Some(&CLTU_TAIL[..]) {
        children.push(Field::new("tail sequence", at * 8, 64, hex_tight(&CLTU_TAIL), "").checked(Check::Ok("found".into())).linked(link("Tail Sequence")));
    } else {
        children.push(Field::new("tail sequence", at * 8, 0, "", "").checked(Check::Warn("not found".into())));
    }
    Some((Field::group("CLTU", children), data))
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
        let (f, data) = cltu_decode(&cltu).unwrap();
        assert!(!f.any_bad());
        assert_eq!(&data[..16], &frame[..]);
        let mut broken = cltu.clone();
        broken[5] ^= 0x04;
        assert!(cltu_decode(&broken).unwrap().0.any_bad());
        // Every parity octet ends in the filler bit 0.
        assert!(cltu[2..cltu.len() - 8].chunks(8).all(|b| b[7] & 1 == 0));
    }
}
