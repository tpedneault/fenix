//! The two packet error control codes PUS allows, and the frame one.

/// CRC-16-CCITT as CCSDS and PUS use it: polynomial `x^16+x^12+x^5+1`
/// (0x1021), preset to all ones, no reflection, no final XOR.
pub fn ccitt16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// The ISO checksum (ISO 8473's, as ECSS PUS defines it) for `bytes`,
/// which will be followed by the two check octets it returns.
pub fn iso_checksum(bytes: &[u8]) -> [u8; 2] {
    let (mut c0, mut c1) = (0u32, 0u32);
    for &b in bytes {
        c0 = (c0 + b as u32) % 255;
        c1 = (c1 + c0) % 255;
    }
    // ISO 8473: with the check octets zeroed at the end (n = L-1, so
    // L-n = 1), X = (L-n)*C0' - C1' and Y = C1' - (L-n+1)*C0', where the
    // primed sums include the two zero octets: C0' = C0, C1' = C1 + 2*C0.
    // That leaves X = -(C0 + C1) and Y = C1, mod 255.
    let x = (510 - c0 - c1) % 255;
    let y = c1 % 255;
    let fix = |v: u32| if v == 0 { 255 } else { v as u8 };
    [fix(x), fix(y)]
}

/// Whether `bytes`, ending in their ISO check octets, add up.
pub fn iso_checksum_ok(bytes: &[u8]) -> bool {
    let (mut c0, mut c1) = (0u32, 0u32);
    for &b in bytes {
        c0 = (c0 + b as u32) % 255;
        c1 = (c1 + c0) % 255;
    }
    c0 == 0 && c1 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ccitt_check_value_and_the_example_packet() {
        assert_eq!(ccitt16(b"123456789"), 0x29B1);
        let pkt = [
            0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16, 0x20, 0x03, 0x19, 0x00, 0x42, 0x00, 0x00, 0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00, 0x00, 0x01, 0x01, 0x0B, 0xB8, 0x0A,
            0x28, 0x02,
        ];
        assert_eq!(ccitt16(&pkt), 0x60E5);
    }

    #[test]
    fn an_iso_checksum_makes_the_message_add_up() {
        for msg in [&b"123456789"[..], &[0u8, 0, 0][..], &[0xFF; 40][..], b"a"] {
            let mut m = msg.to_vec();
            m.extend(iso_checksum(msg));
            assert!(iso_checksum_ok(&m), "{msg:?}");
            m[0] ^= 0x10;
            assert!(!iso_checksum_ok(&m));
        }
    }
}
