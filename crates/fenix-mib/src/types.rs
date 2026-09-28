//! SCOS-2000's parameter type codes (PTC) and format codes (PFC), per
//! ECSS-E-70-41, turned into what a person reads: `PTC 3 / PFC 12` is a
//! `uint16`, `PTC 5 / PFC 2` a `float64`. The code is kept beside the
//! name wherever it's shown; this only decodes it.

/// A decoded type: its name, and its width in bits when it has a fixed
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub name: String,
    pub bits: Option<u32>,
}

/// Bits of an unsigned or signed integer of format `pfc`.
fn integer_bits(pfc: u32) -> Option<u32> {
    match pfc {
        0..=12 => Some(pfc + 4),
        13 => Some(24),
        14 => Some(32),
        15 => Some(48),
        16 => Some(64),
        _ => None,
    }
}

/// CUC's coarse and fine octets for a time of format `pfc` (3..=18).
fn cuc(pfc: u32) -> Option<(u32, u32)> {
    (3..=18).contains(&pfc).then(|| ((pfc - 3) / 4 + 1, (pfc - 3) % 4))
}

/// `ptc`/`pfc` as a readable type. Codes that don't parse, or that the
/// standard doesn't define, come back as `PTC x/y`.
pub fn decode(ptc: &str, pfc: &str) -> TypeInfo {
    let raw = || TypeInfo { name: format!("PTC {}/{}", ptc.trim(), pfc.trim()), bits: None };
    let (Ok(ptc), Ok(pfc)) = (ptc.trim().parse::<u32>(), pfc.trim().parse::<u32>()) else {
        return if ptc.trim().is_empty() { TypeInfo { name: String::new(), bits: None } } else { raw() };
    };
    let t = |name: String, bits: Option<u32>| TypeInfo { name, bits };
    match ptc {
        1 if pfc == 0 => t("bool".into(), Some(1)),
        2 if (1..=32).contains(&pfc) => t(format!("enum{pfc}"), Some(pfc)),
        3 => integer_bits(pfc).map_or_else(raw, |b| t(format!("uint{b}"), Some(b))),
        4 => integer_bits(pfc).map_or_else(raw, |b| t(format!("int{b}"), Some(b))),
        5 => match pfc {
            1 => t("float32".into(), Some(32)),
            2 => t("float64".into(), Some(64)),
            3 => t("MIL-1750A float".into(), Some(32)),
            4 => t("MIL-1750A float48".into(), Some(48)),
            _ => raw(),
        },
        6 if pfc == 0 => t("bit string".into(), None),
        6 => t(format!("bit string {pfc}"), Some(pfc)),
        7 if pfc == 0 => t("octets".into(), None),
        7 => t(format!("octets[{pfc}]"), Some(pfc * 8)),
        8 if pfc == 0 => t("string".into(), None),
        8 => t(format!("char[{pfc}]"), Some(pfc * 8)),
        9 => match pfc {
            0 => t("absolute time".into(), None),
            1 => t("absolute time (CDS 6)".into(), Some(48)),
            2 => t("absolute time (CDS 8)".into(), Some(64)),
            _ => cuc(pfc).map_or_else(raw, |(c, f)| t(format!("absolute time (CUC {c}+{f})"), Some((c + f) * 8))),
        },
        10 => cuc(pfc).map_or_else(raw, |(c, f)| t(format!("relative time (CUC {c}+{f})"), Some((c + f) * 8))),
        11 => t("deduced".into(), None),
        13 => t("saved parameter".into(), None),
        _ => raw(),
    }
}

/// Whether values of this type are numbers (to range-check, and to
/// line up right).
pub fn numeric(ptc: &str) -> bool {
    matches!(ptc.trim(), "1" | "2" | "3" | "4" | "5")
}

/// A number as the MIB writes it, or as someone types it: decimal, or
/// hex with `0x`.
pub fn parse_int(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return i64::from_str_radix(hex, 16).ok();
    }
    s.parse().ok()
}

/// An APID in hex (`0x3F2`) or decimal; the text as it was when it isn't
/// a number.
pub fn apid(raw: &str, hex: bool) -> String {
    match parse_int(raw) {
        Some(n) if hex => format!("0x{n:X}"),
        Some(n) => n.to_string(),
        None => raw.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_floats_and_times_read_as_types() {
        assert_eq!(decode("3", "12"), TypeInfo { name: "uint16".into(), bits: Some(16) });
        assert_eq!(decode("3", "4").name, "uint8");
        assert_eq!(decode("4", "14").name, "int32");
        assert_eq!(decode("5", "2").name, "float64");
        assert_eq!(decode("1", "0").bits, Some(1));
        assert_eq!(decode("2", "8").name, "enum8");
        assert_eq!(decode("8", "10").name, "char[10]");
        assert_eq!(decode("9", "18"), TypeInfo { name: "absolute time (CUC 4+3)".into(), bits: Some(56) });
        assert_eq!(decode("10", "3").name, "relative time (CUC 1+0)");
    }

    #[test]
    fn unknown_codes_stay_codes_and_empty_stays_empty() {
        assert_eq!(decode("5", "9").name, "PTC 5/9");
        assert_eq!(decode("x", "1").name, "PTC x/1");
        assert_eq!(decode("", "").name, "");
    }

    #[test]
    fn apids_and_numbers_read_either_way() {
        assert_eq!(apid("1010", true), "0x3F2");
        assert_eq!(apid("0x3F2", false), "1010");
        assert_eq!(apid("n/a", true), "n/a");
        assert_eq!(parse_int("0X10"), Some(16));
    }
}
