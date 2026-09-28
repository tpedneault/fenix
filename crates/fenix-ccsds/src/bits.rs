//! Big-endian bit fields, the way every CCSDS header lays them out: bit 0
//! is the most significant bit of the first octet.

/// `bits` bits (up to 64) starting at bit `at`; `None` past the end.
pub fn get(bytes: &[u8], at: usize, bits: usize) -> Option<u64> {
    if bits == 0 {
        return Some(0);
    }
    if bits > 64 || at + bits > bytes.len() * 8 {
        return None;
    }
    let mut v: u64 = 0;
    for i in at..at + bits {
        let bit = (bytes[i / 8] >> (7 - i % 8)) & 1;
        v = (v << 1) | bit as u64;
    }
    Some(v)
}

/// Writes the low `bits` bits of `value` at bit `at`, growing `bytes` as
/// needed.
pub fn put(bytes: &mut Vec<u8>, at: usize, bits: usize, value: u64) {
    let end = (at + bits).div_ceil(8);
    if bytes.len() < end {
        bytes.resize(end, 0);
    }
    for k in 0..bits {
        let i = at + k;
        let bit = ((value >> (bits - 1 - k)) & 1) as u8;
        let mask = 1 << (7 - i % 8);
        if bit == 1 {
            bytes[i / 8] |= mask;
        } else {
            bytes[i / 8] &= !mask;
        }
    }
}

/// `value` read as a two's-complement number of `bits` bits.
pub fn signed(value: u64, bits: usize) -> i64 {
    if bits == 0 || bits >= 64 {
        return value as i64;
    }
    let sign = 1u64 << (bits - 1);
    if value & sign != 0 {
        (value as i64) - (1i64 << bits)
    } else {
        value as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_read_and_write_across_octets() {
        let b = [0x0B, 0xF2, 0xC1, 0x23];
        assert_eq!(get(&b, 0, 3), Some(0));
        assert_eq!(get(&b, 4, 1), Some(1));
        assert_eq!(get(&b, 5, 11), Some(0x3F2));
        assert_eq!(get(&b, 16, 2), Some(3));
        assert_eq!(get(&b, 18, 14), Some(0x0123));
        assert_eq!(get(&b, 30, 3), None);
        let mut out = Vec::new();
        put(&mut out, 5, 11, 0x3F2);
        put(&mut out, 4, 1, 1);
        assert_eq!(out, vec![0x0B, 0xF2]);
        assert_eq!(signed(0xFFF, 12), -1);
        assert_eq!(signed(0x7FF, 12), 2047);
    }
}
