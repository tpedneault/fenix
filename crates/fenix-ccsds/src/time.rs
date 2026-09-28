//! CCSDS 301.0-B time codes -- CUC (unsegmented) and CDS (day segmented),
//! with or without their P-field -- against an epoch (1958 TAI, the level
//! 1 epoch, or a mission's own) and leap seconds. Instants are carried as
//! TAI seconds since 1958-01-01, the one scale that never jumps.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, Timelike};

use crate::bits;
use crate::field::{hex_tight, Field, Link};

pub const STANDARD: &str = "CCSDS 301.0-B";

/// TAI seconds since 1958-01-01 00:00:00 TAI.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Tai(pub f64);

fn base_1958() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(1958, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()
}

/// TAI - UTC, by the UTC date it starts on.
#[derive(Debug, Clone, PartialEq)]
pub struct LeapTable(Vec<(NaiveDate, i64)>);

impl Default for LeapTable {
    fn default() -> Self {
        let d = |y, m| NaiveDate::from_ymd_opt(y, m, 1).unwrap();
        LeapTable(vec![
            (d(1972, 1), 10), (d(1972, 7), 11), (d(1973, 1), 12), (d(1974, 1), 13), (d(1975, 1), 14), (d(1976, 1), 15), (d(1977, 1), 16),
            (d(1978, 1), 17), (d(1979, 1), 18), (d(1980, 1), 19), (d(1981, 7), 20), (d(1982, 7), 21), (d(1983, 7), 22), (d(1985, 7), 23),
            (d(1988, 1), 24), (d(1990, 1), 25), (d(1991, 1), 26), (d(1992, 7), 27), (d(1993, 7), 28), (d(1994, 7), 29), (d(1996, 1), 30),
            (d(1997, 7), 31), (d(1999, 1), 32), (d(2006, 1), 33), (d(2009, 1), 34), (d(2012, 7), 35), (d(2015, 7), 36), (d(2017, 1), 37),
        ])
    }
}

impl LeapTable {
    /// A table from lines of `YYYY-MM-DD N` (TAI - UTC from that date);
    /// `#` starts a comment.
    pub fn parse(text: &str) -> Result<LeapTable, String> {
        let mut rows = Vec::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(date), Some(n)) = (parts.next(), parts.next()) else { return Err(format!("line {}: expected a date and a count", i + 1)) };
            let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|e| format!("line {}: {e}", i + 1))?;
            let n: i64 = n.parse().map_err(|_| format!("line {}: {n} isn't a number", i + 1))?;
            rows.push((date, n));
        }
        rows.sort();
        if rows.is_empty() {
            return Err("no leap seconds in it".into());
        }
        Ok(LeapTable(rows))
    }

    /// TAI - UTC on `date` (10 before 1972).
    pub fn offset(&self, date: NaiveDate) -> i64 {
        self.0.iter().rev().find(|(d, _)| *d <= date).map(|(_, n)| *n).unwrap_or(10)
    }
}

pub fn utc_to_tai(utc: NaiveDateTime, leap: &LeapTable) -> Tai {
    let secs = (utc - base_1958()).num_microseconds().unwrap_or(0) as f64 / 1e6;
    Tai(secs + leap.offset(utc.date()) as f64)
}

pub fn tai_to_utc(t: Tai, leap: &LeapTable) -> NaiveDateTime {
    let approx = base_1958() + Duration::microseconds(((t.0 - 37.0) * 1e6).round() as i64);
    let mut off = leap.offset(approx.date());
    let utc = base_1958() + Duration::microseconds(((t.0 - off as f64) * 1e6).round() as i64);
    let again = leap.offset(utc.date());
    if again != off {
        off = again;
        return base_1958() + Duration::microseconds(((t.0 - off as f64) * 1e6).round() as i64);
    }
    utc
}

/// GPS time: seconds since 1980-01-06 (GPS = TAI - 19 s).
pub fn tai_to_gps(t: Tai) -> f64 {
    let gps_epoch_tai = (NaiveDate::from_ymd_opt(1980, 1, 6).unwrap().and_hms_opt(0, 0, 0).unwrap() - base_1958()).num_seconds() as f64 + 19.0;
    t.0 - gps_epoch_tai
}

pub fn gps_to_tai(gps: f64) -> Tai {
    Tai(gps + tai_to_gps(Tai(0.0)).abs())
}

/// `2026-09-27 14:32:05.500`.
pub fn format_utc(utc: NaiveDateTime) -> String {
    utc.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

/// `2026-270T14:32:05.500Z`, CCSDS ASCII time code B.
pub fn format_doy(utc: NaiveDateTime) -> String {
    format!("{:04}-{:03}T{:02}:{:02}:{:02}.{:03}Z", utc.year(), utc.ordinal(), utc.hour(), utc.minute(), utc.second(), utc.nanosecond() / 1_000_000)
}

/// A UTC time as people type it: `2026-09-27 14:32:05.5`, the `T` form,
/// the day-of-year form, a date alone.
pub fn parse_utc(text: &str) -> Option<NaiveDateTime> {
    let t = text.trim().trim_end_matches('Z').replace('T', " ");
    for f in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%j %H:%M:%S%.f", "%Y-%j %H:%M:%S"] {
        if let Ok(v) = NaiveDateTime::parse_from_str(&t, f) {
            return Some(v);
        }
    }
    NaiveDate::parse_from_str(&t, "%Y-%m-%d").ok().and_then(|d| d.and_hms_opt(0, 0, 0))
}

/// Which scale an epoch is given in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scale {
    Tai,
    Utc,
    Gps,
}

/// What on-board times count from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Epoch {
    /// The epoch, in TAI seconds since 1958.
    pub tai: Tai,
    /// The level 1 epoch (1958 TAI) -- a P-field says so.
    pub level1: bool,
}

impl Default for Epoch {
    fn default() -> Self {
        Epoch { tai: Tai(0.0), level1: true }
    }
}

impl Epoch {
    /// `1958-01-01 TAI` (the default), `2000-01-01T12:00:00 TT`... any
    /// date and time, then `TAI`, `UTC` or `GPS` (TAI when left out).
    pub fn parse(text: &str, leap: &LeapTable) -> Result<Epoch, String> {
        let text = text.trim();
        let (when, scale) = match text.rsplit_once(' ') {
            Some((w, s)) if ["TAI", "UTC", "GPS"].contains(&s.to_ascii_uppercase().as_str()) => (w, s.to_ascii_uppercase()),
            _ => (text, "TAI".to_string()),
        };
        let at = parse_utc(when).ok_or_else(|| format!("{when} isn't a date"))?;
        let naive = (at - base_1958()).num_microseconds().unwrap_or(0) as f64 / 1e6;
        let tai = match scale.as_str() {
            "UTC" => utc_to_tai(at, leap),
            "GPS" => Tai(naive + 19.0),
            _ => Tai(naive),
        };
        Ok(Epoch { tai, level1: tai.0 == 0.0 && scale == "TAI" })
    }
}

/// A time correlation: the epoch on-board times really count from, found
/// from one on-board time and the UTC it matched -- on-board clocks drift
/// and are reset, so what they count from isn't the nominal epoch.
/// `text` is `on-board = UTC`: the on-board time as the mission writes a
/// CUC (`814B878A.8000`, hex) or in seconds since the nominal epoch
/// (`2169210762.5`), then a UTC date and time.
pub fn correlate(text: &str, fmt: TimeFormat, leap: &LeapTable) -> Result<Epoch, String> {
    let (onboard, utc) = text.split_once('=').ok_or("an on-board time = the UTC it matched")?;
    let utc = parse_utc(utc).ok_or_else(|| format!("{} isn't a date and time", utc.trim()))?;
    let onboard = onboard.trim();
    let hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    let secs = match (fmt, onboard.split_once('.')) {
        // CUC as it's written: the coarse octets in hex, then the fine.
        (TimeFormat::Cuc { coarse, .. }, Some((c, f))) if hex(c) && hex(f) && c.len() == 2 * coarse as usize => {
            let c = u64::from_str_radix(c, 16).map_err(|e| e.to_string())? as f64;
            let f = u64::from_str_radix(f, 16).map_err(|e| e.to_string())? as f64 / 16f64.powi(f.len() as i32);
            c + f
        }
        (TimeFormat::Cuc { coarse, .. }, None) if hex(onboard) && onboard.len() == 2 * coarse as usize && onboard.chars().any(|c| c.is_ascii_alphabetic()) => {
            u64::from_str_radix(onboard, 16).map_err(|e| e.to_string())? as f64
        }
        _ => onboard.parse::<f64>().map_err(|_| format!("{onboard} isn't an on-board time (hex as the mission writes it, or seconds)"))?,
    };
    Ok(Epoch { tai: Tai(utc_to_tai(utc, leap).0 - secs), level1: false })
}

/// A time code's layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    Cuc { coarse: u8, fine: u8, pfield: bool },
    Cds { days24: bool, submilli: u8, pfield: bool },
    None,
}

impl TimeFormat {
    /// `cuc 4.2`, `cuc 4.2 p` (with a P-field), `cds 16`, `cds 24 2`
    /// (sub-milliseconds in 2 octets), `none`.
    pub fn parse(text: &str) -> Result<TimeFormat, String> {
        let words: Vec<String> = text.split_whitespace().map(str::to_ascii_lowercase).collect();
        let p = words.iter().any(|w| w == "p");
        match words.first().map(String::as_str) {
            Some("none") | None => Ok(TimeFormat::None),
            Some("cuc") => {
                let spec = words.get(1).map(String::as_str).unwrap_or("4.2");
                let (c, f) = spec.split_once('.').unwrap_or((spec, "0"));
                let coarse: u8 = c.parse().map_err(|_| format!("{c} coarse octets?"))?;
                let fine: u8 = f.parse().map_err(|_| format!("{f} fine octets?"))?;
                if !(1..=7).contains(&coarse) || fine > 10 {
                    return Err("CUC has 1 to 7 coarse and up to 10 fine octets".into());
                }
                Ok(TimeFormat::Cuc { coarse, fine, pfield: p })
            }
            Some("cds") => {
                let days24 = words.get(1).is_some_and(|w| w == "24");
                let submilli = words.get(2).and_then(|w| w.parse().ok()).unwrap_or(0);
                if ![0, 2, 4].contains(&submilli) {
                    return Err("CDS sub-milliseconds are 0, 2 or 4 octets".into());
                }
                Ok(TimeFormat::Cds { days24, submilli, pfield: p })
            }
            Some(other) => Err(format!("{other}: cuc, cds or none")),
        }
    }

    /// Octets it takes, P-field included.
    pub fn len(&self) -> usize {
        match *self {
            TimeFormat::Cuc { coarse, fine, pfield } => coarse as usize + fine as usize + if pfield { if coarse > 4 || fine > 3 { 2 } else { 1 } } else { 0 },
            TimeFormat::Cds { days24, submilli, pfield } => (if days24 { 3 } else { 2 }) + 4 + submilli as usize + pfield as usize,
            TimeFormat::None => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn label(&self) -> String {
        match *self {
            TimeFormat::Cuc { coarse, fine, .. } => format!("CUC {coarse}+{fine}"),
            TimeFormat::Cds { days24, submilli, .. } => format!("CDS {}{}", if days24 { 24 } else { 16 }, if submilli > 0 { format!("+{submilli}") } else { String::new() }),
            TimeFormat::None => "no time".into(),
        }
    }
}

/// A decoded time: when, and its fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub tai: Tai,
    pub field: Field,
    pub len: usize,
}

/// Reads a CUC P-field: its format.
fn cuc_pfield(bytes: &[u8]) -> Option<(u8, u8, usize, bool)> {
    let p = *bytes.first()?;
    let id = (p >> 4) & 0b111;
    if id != 0b001 && id != 0b010 {
        return None;
    }
    let mut coarse = ((p >> 2) & 0b11) + 1;
    let mut fine = p & 0b11;
    let mut len = 1;
    if p & 0x80 != 0 {
        let q = *bytes.get(1)?;
        coarse += (q >> 5) & 0b11;
        fine += (q >> 2) & 0b111;
        len = 2;
    }
    Some((coarse, fine, len, id == 0b001))
}

/// Decodes `bytes` as `format` from `epoch`, starting at bit `at` of the
/// tree it belongs to.
pub fn decode(bytes: &[u8], format: TimeFormat, epoch: Epoch, leap: &LeapTable, at: usize) -> Option<Decoded> {
    let std = |h| Link::Standard(STANDARD, h);
    match format {
        TimeFormat::None => None,
        TimeFormat::Cuc { coarse, fine, pfield } => {
            let (coarse, fine, plen, level1) = if pfield { cuc_pfield(bytes)? } else { (coarse, fine, 0, epoch.level1) };
            let t = &bytes.get(plen..plen + coarse as usize + fine as usize)?;
            let c = bits::get(t, 0, coarse as usize * 8)?;
            let f = bits::get(t, coarse as usize * 8, fine as usize * 8)?;
            let secs = c as f64 + if fine > 0 { f as f64 / 256f64.powi(fine as i32) } else { 0.0 };
            let base = if level1 { Tai(0.0) } else { epoch.tai };
            let tai = Tai(base.0 + secs);
            let len = plen + coarse as usize + fine as usize;
            let mut children = Vec::new();
            if plen > 0 {
                children.push(Field::new("P-field", at, plen * 8, hex_tight(&bytes[..plen]), format!("CUC {coarse}+{fine}, {}", if level1 { "1958 epoch" } else { "agency epoch" })).linked(std("Preamble Field")));
            }
            children.push(Field::new("coarse", at + plen * 8, coarse as usize * 8, format!("0x{c:0w$X}", w = coarse as usize * 2), format!("{c} s")));
            if fine > 0 {
                children.push(Field::new("fine", at + (plen + coarse as usize) * 8, fine as usize * 8, format!("0x{f:0w$X}", w = fine as usize * 2), format!("{:.6} s", secs.fract())));
            }
            let utc = tai_to_utc(tai, leap);
            let mut field = Field::group(format!("time (CUC {coarse}+{fine})"), children);
            field.raw = hex_tight(&bytes[plen..len]);
            field.value = format!("{} UTC", format_utc(utc));
            field.link = Some(std("CCSDS Unsegmented Time Code (CUC)"));
            Some(Decoded { tai, field, len })
        }
        TimeFormat::Cds { days24, submilli, pfield } => {
            let (days24, submilli, plen, level1) = if pfield {
                let p = *bytes.first()?;
                if (p >> 4) & 0b111 != 0b100 {
                    return None;
                }
                (p & 0b100 != 0, match p & 0b11 { 1 => 2, 2 => 4, _ => 0 }, 1, p & 0b1000 == 0)
            } else {
                (days24, submilli, 0, epoch.level1)
            };
            let dl = if days24 { 3 } else { 2 };
            let t = bytes.get(plen..plen + dl + 4 + submilli as usize)?;
            let days = bits::get(t, 0, dl * 8)?;
            let ms = bits::get(t, dl * 8, 32)?;
            let sub = bits::get(t, (dl + 4) * 8, submilli as usize * 8)?;
            let sub_s = match submilli {
                2 => sub as f64 * 1e-6,
                4 => sub as f64 * 1e-12,
                _ => 0.0,
            };
            let secs = days as f64 * 86400.0 + ms as f64 / 1000.0 + sub_s;
            let base = if level1 { Tai(0.0) } else { epoch.tai };
            let tai = Tai(base.0 + secs);
            let len = plen + dl + 4 + submilli as usize;
            let mut children = vec![
                Field::new("days", at + plen * 8, dl * 8, days.to_string(), format!("{days} days")),
                Field::new("ms of day", at + (plen + dl) * 8, 32, ms.to_string(), format!("{:.3} s", ms as f64 / 1000.0)),
            ];
            if submilli > 0 {
                children.push(Field::new("sub-ms", at + (plen + dl + 4) * 8, submilli as usize * 8, sub.to_string(), ""));
            }
            let mut field = Field::group(format!("time (CDS {})", if days24 { 24 } else { 16 }), children);
            field.raw = hex_tight(&bytes[plen..len]);
            field.value = format!("{} UTC", format_utc(tai_to_utc(tai, leap)));
            field.link = Some(std("CCSDS Day Segmented Time Code (CDS)"));
            Some(Decoded { tai, field, len })
        }
    }
}

/// `tai` in `format`, counted from `epoch`, P-field included when the
/// format has one.
pub fn encode(tai: Tai, format: TimeFormat, epoch: Epoch) -> Vec<u8> {
    let secs = (tai.0 - epoch.tai.0).max(0.0);
    let mut out = Vec::new();
    match format {
        TimeFormat::None => {}
        TimeFormat::Cuc { coarse, fine, pfield } => {
            if pfield {
                let id: u8 = if epoch.level1 { 0b001 } else { 0b010 };
                let ext = coarse > 4 || fine > 3;
                let c = coarse.min(4) - 1;
                let f = fine.min(3);
                out.push(((ext as u8) << 7) | (id << 4) | (c << 2) | f);
                if ext {
                    out.push(((coarse.saturating_sub(4)) << 5) | (fine.saturating_sub(3) << 2));
                }
            }
            let c = secs.trunc() as u64;
            let f = (secs.fract() * 256f64.powi(fine as i32)).round() as u64;
            let at = out.len() * 8;
            bits::put(&mut out, at, coarse as usize * 8, c);
            let at = out.len() * 8;
            bits::put(&mut out, at, fine as usize * 8, f.min(256u64.saturating_pow(fine as u32).saturating_sub(1)));
        }
        TimeFormat::Cds { days24, submilli, pfield } => {
            if pfield {
                let sm = match submilli {
                    2 => 1,
                    4 => 2,
                    _ => 0,
                };
                out.push((0b100 << 4) | ((!epoch.level1 as u8) << 3) | ((days24 as u8) << 2) | sm);
            }
            let days = (secs / 86400.0).floor() as u64;
            let rest = secs - days as f64 * 86400.0;
            let ms = (rest * 1000.0).floor() as u64;
            let dl = if days24 { 24 } else { 16 };
            let at = out.len() * 8;
            bits::put(&mut out, at, dl, days);
            let at = out.len() * 8;
            bits::put(&mut out, at, 32, ms);
            let sub = rest * 1000.0 - ms as f64;
            let at = out.len() * 8;
            match submilli {
                2 => bits::put(&mut out, at, 16, (sub * 1000.0).round() as u64),
                4 => bits::put(&mut out, at, 32, (sub * 1e9).round() as u64),
                _ => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_correlation_moves_the_epoch_to_where_the_clock_really_counts_from() {
        let leap = LeapTable::default();
        let fmt = TimeFormat::Cuc { coarse: 4, fine: 2, pfield: false };
        // The example packet's time, which reads 14:32:05.500 from 1958:
        // say the clock was 10 s fast.
        let e = correlate("814B878A.8000 = 2026-09-27 14:31:55.5", fmt, &leap).unwrap();
        let bytes = [0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00];
        let d = decode(&bytes, fmt, e, &leap, 0).unwrap();
        assert_eq!(format_utc(tai_to_utc(d.tai, &leap)), "2026-09-27 14:31:55.500");
        assert!(!e.level1);
        let by_seconds = correlate("2169210762.5 = 2026-09-27 14:31:55.5", fmt, &leap).unwrap();
        assert!((by_seconds.tai.0 - e.tai.0).abs() < 1e-6);
        assert!(correlate("814B878A.8000", fmt, &leap).is_err());
        assert!(correlate("soon = 2026-09-27", fmt, &leap).is_err());
    }

    use super::*;

    fn utc(s: &str) -> NaiveDateTime {
        parse_utc(s).unwrap()
    }

    #[test]
    fn the_example_cuc_is_the_example_time() {
        let leap = LeapTable::default();
        let d = decode(&[0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00], TimeFormat::parse("cuc 4.2").unwrap(), Epoch::default(), &leap, 0).unwrap();
        assert_eq!(format_utc(tai_to_utc(d.tai, &leap)), "2026-09-27 14:32:05.500");
        assert_eq!(d.len, 6);
        let back = encode(utc_to_tai(utc("2026-09-27T14:32:05.5Z"), &leap), TimeFormat::parse("cuc 4.2").unwrap(), Epoch::default());
        assert_eq!(back, vec![0x81, 0x4B, 0x87, 0x8A, 0x80, 0x00]);
        assert_eq!(format_doy(utc("2026-09-27 14:32:05.5")), "2026-270T14:32:05.500Z");
    }

    #[test]
    fn p_fields_and_cds_round_trip() {
        let leap = LeapTable::default();
        let t = utc_to_tai(utc("2026-09-27 14:32:05.5"), &leap);
        for spec in ["cuc 4.2 p", "cuc 4.3", "cds 16", "cds 24 2 p", "cuc 5.3 p"] {
            let f = TimeFormat::parse(spec).unwrap();
            let bytes = encode(t, f, Epoch::default());
            assert_eq!(bytes.len(), f.len(), "{spec}");
            let d = decode(&bytes, f, Epoch::default(), &leap, 0).unwrap();
            assert!((d.tai.0 - t.0).abs() < 0.001, "{spec}: {} vs {}", d.tai.0, t.0);
        }
        let cds = encode(t, TimeFormat::parse("cds 16").unwrap(), Epoch::default());
        assert_eq!(bits::get(&cds, 0, 16), Some(25106));
        assert_eq!(bits::get(&cds, 16, 32), Some(52_362_500));
    }

    #[test]
    fn leap_seconds_epochs_and_gps() {
        let leap = LeapTable::default();
        assert_eq!(leap.offset(NaiveDate::from_ymd_opt(2026, 9, 27).unwrap()), 37);
        assert_eq!(leap.offset(NaiveDate::from_ymd_opt(1998, 6, 1).unwrap()), 31);
        assert_eq!(leap.offset(NaiveDate::from_ymd_opt(1999, 6, 1).unwrap()), 32);
        let t = utc_to_tai(utc("2026-09-27 14:32:05.5"), &leap);
        let gps = tai_to_gps(t);
        assert_eq!(((gps / 604800.0).floor() as i64, gps % 604800.0), (2438, 52343.5));
        let e = Epoch::parse("2000-01-01T12:00:00 TAI", &leap).unwrap();
        assert!(!e.level1);
        let own = decode(&encode(t, TimeFormat::parse("cuc 4.2").unwrap(), e), TimeFormat::parse("cuc 4.2").unwrap(), e, &leap, 0).unwrap();
        assert!((own.tai.0 - t.0).abs() < 0.001);
        let table = LeapTable::parse("# test\n2017-01-01 37\n2030-01-01 38\n").unwrap();
        assert_eq!(table.offset(NaiveDate::from_ymd_opt(2031, 1, 1).unwrap()), 38);
        assert!(TimeFormat::parse("cds 16 3").is_err());
    }
}
