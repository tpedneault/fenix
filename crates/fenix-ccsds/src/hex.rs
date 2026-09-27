//! Bytes out of text, however hex gets written: `0B F2 C1`, `0x0BF2C1`,
//! `0bf2c1`, `\x0b\xf2`, `{0x0B, 0xF2}`, `0B:F2`, a line with an offset
//! column (`0010: 8A 80 ...`).

/// The bytes `text` spells, or `None` when it isn't hex. Offsets at the
/// start of lines (`0010:`, `00000010 `) are skipped when every line has
/// one.
pub fn parse(text: &str) -> Option<Vec<u8>> {
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let offset_col = lines.len() > 1 && lines.iter().all(|l| l.split_once([':', ' ']).is_some_and(|(o, _)| o.len() >= 4 && o.chars().all(|c| c.is_ascii_hexdigit())));
    let mut digits = String::new();
    for line in lines {
        let body = if offset_col { line.split_once([':', ' ']).map(|(_, b)| b).unwrap_or("") } else { line };
        // An ASCII column after two spaces (`8A 80  ..(.`) is dropped.
        let body = if offset_col { body.split("  ").find(|s| !s.trim().is_empty()).unwrap_or("") } else { body };
        let cleaned = body.replace("0x", " ").replace("0X", " ").replace("\\x", " ");
        for c in cleaned.chars() {
            if c.is_ascii_hexdigit() {
                digits.push(c);
            } else if c.is_whitespace() || matches!(c, ',' | ':' | '-' | '{' | '}' | '[' | ']' | '(' | ')' | ';' | '"' | '\'' | '_') {
                // separators
            } else {
                return None;
            }
        }
    }
    if digits.len() < 2 || !digits.len().is_multiple_of(2) {
        return None;
    }
    (0..digits.len()).step_by(2).map(|i| u8::from_str_radix(&digits[i..i + 2], 16).ok()).collect()
}

/// The hex run around column `col` of `line`: the longest stretch of hex
/// digits, spaces and the usual separators that contains it -- what
/// `SPC k d` takes when nothing is selected.
pub fn around(line: &str, col: usize) -> Option<(std::ops::Range<usize>, Vec<u8>)> {
    let chars: Vec<char> = line.chars().collect();
    if col >= chars.len() {
        return None;
    }
    let part = |c: char| c.is_ascii_hexdigit() || c == ' ' || c == 'x' || c == 'X' || c == ',';
    if !part(chars[col]) || chars[col] == ' ' && !chars.get(col + 1).is_some_and(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut start = col;
    while start > 0 && part(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < chars.len() && part(chars[end]) {
        end += 1;
    }
    // Trim to hex digits at both ends.
    while start < end && !chars[start].is_ascii_hexdigit() {
        start += 1;
    }
    while end > start && !chars[end - 1].is_ascii_hexdigit() {
        end -= 1;
    }
    let text: String = chars[start..end].iter().collect();
    let bytes = parse(&text)?;
    (bytes.len() >= 4).then_some((start..end, bytes))
}

/// `bytes` as a literal in a language: `"c"` gives `{0x0B, 0xF2}`,
/// `"py"` `bytes([0x0B, 0xF2])`, anything else spaced hex.
pub fn literal(bytes: &[u8], language: &str) -> String {
    let list = bytes.iter().map(|b| format!("0x{b:02X}")).collect::<Vec<_>>().join(", ");
    match language {
        "c" | "cpp" | "h" | "rs" => format!("{{{list}}}"),
        "py" => format!("bytes([{list}])"),
        _ => crate::field::hex(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_usual_spelling_reads() {
        let want = vec![0x0B, 0xF2, 0xC1, 0x23];
        for s in ["0B F2 C1 23", "0x0BF2C123", "0bf2c123", "\\x0b\\xf2\\xc1\\x23", "{0x0B, 0xF2, 0xC1, 0x23}", "0B:F2:C1:23", "0bf2 c123"] {
            assert_eq!(parse(s), Some(want.clone()), "{s}");
        }
        assert_eq!(parse("0000: 0B F2\n0002: C1 23"), Some(want.clone()));
        assert_eq!(parse("not hex"), None);
        assert_eq!(parse("ABC"), None);
    }

    #[test]
    fn the_run_around_the_cursor_is_found() {
        let line = "rx 12:00:01 pkt=0B F2 C1 23 00 16 ok";
        let (range, bytes) = around(line, 20).unwrap();
        assert_eq!(bytes, vec![0x0B, 0xF2, 0xC1, 0x23, 0x00, 0x16]);
        assert_eq!(&line[range], "0B F2 C1 23 00 16");
        assert!(around(line, 1).is_none());
        assert_eq!(literal(&[1, 2], "py"), "bytes([0x01, 0x02])");
        assert_eq!(literal(&[1, 2], "c"), "{0x01, 0x02}");
    }
}
