//! What a text file's bytes mean. Fenix edits Unicode text; a file that
//! isn't UTF-8 is decoded when it's read and written back the way it came.

use encoding_rs::{Encoding, EUC_JP, EUC_KR, GBK, ISO_8859_2, ISO_8859_15, KOI8_R, SHIFT_JIS, WINDOWS_1250, WINDOWS_1251, WINDOWS_1252};

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];
const UTF16_LE_BOM: &[u8] = &[0xFF, 0xFE];
const UTF16_BE_BOM: &[u8] = &[0xFE, 0xFF];

/// How much of a file's start is looked at to say whether it's text.
pub const TEXT_SNIFF_BYTES: usize = 8192;

/// A text file's encoding. Its byte order mark, if any, is part of this
/// and never of the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextEncoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// One of the older encodings, where every byte sequence means
    /// something, so it can't be told from the bytes which one it is.
    Legacy(&'static Encoding),
}

impl TextEncoding {
    /// Everything that can be chosen, in the order it's offered.
    pub fn choices() -> Vec<TextEncoding> {
        let mut all = vec![TextEncoding::Utf8, TextEncoding::Utf8Bom, TextEncoding::Utf16Le, TextEncoding::Utf16Be];
        all.extend([WINDOWS_1252, ISO_8859_15, WINDOWS_1250, ISO_8859_2, WINDOWS_1251, KOI8_R, SHIFT_JIS, EUC_JP, GBK, EUC_KR].map(TextEncoding::Legacy));
        all
    }

    /// As the modeline says it.
    pub fn label(self) -> &'static str {
        match self {
            TextEncoding::Utf8 => "UTF-8",
            TextEncoding::Utf8Bom => "UTF-8 BOM",
            TextEncoding::Utf16Le => "UTF-16 LE",
            TextEncoding::Utf16Be => "UTF-16 BE",
            TextEncoding::Legacy(encoding) => encoding.name(),
        }
    }

    /// `bytes` as this encoding. Never fails: what doesn't decode becomes
    /// U+FFFD, which only a Unicode encoding will write back.
    pub fn decode(self, bytes: &[u8]) -> String {
        match self {
            TextEncoding::Utf8 | TextEncoding::Utf8Bom => String::from_utf8_lossy(bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes)).into_owned(),
            TextEncoding::Utf16Le => decode_utf16(bytes.strip_prefix(UTF16_LE_BOM).unwrap_or(bytes), u16::from_le_bytes),
            TextEncoding::Utf16Be => decode_utf16(bytes.strip_prefix(UTF16_BE_BOM).unwrap_or(bytes), u16::from_be_bytes),
            TextEncoding::Legacy(encoding) => encoding.decode_without_bom_handling(bytes).0.into_owned(),
        }
    }

    /// `text` as this encoding, or the first character it can't hold:
    /// writing the rest and losing that one is never what's wanted.
    pub fn encode(self, text: &str) -> Result<Vec<u8>, char> {
        let mut bytes = match self {
            TextEncoding::Utf8 => Vec::with_capacity(text.len()),
            TextEncoding::Utf8Bom => UTF8_BOM.to_vec(),
            TextEncoding::Utf16Le => UTF16_LE_BOM.to_vec(),
            TextEncoding::Utf16Be => UTF16_BE_BOM.to_vec(),
            TextEncoding::Legacy(_) => Vec::new(),
        };
        match self {
            TextEncoding::Utf8 | TextEncoding::Utf8Bom => bytes.extend_from_slice(text.as_bytes()),
            TextEncoding::Utf16Le => bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes)),
            TextEncoding::Utf16Be => bytes.extend(text.encode_utf16().flat_map(u16::to_be_bytes)),
            TextEncoding::Legacy(encoding) => {
                let (encoded, _, had_errors) = encoding.encode(text);
                if had_errors {
                    let lost = text.chars().find(|c| encoding.encode(c.encode_utf8(&mut [0; 4])).2).unwrap_or('\u{FFFD}');
                    return Err(lost);
                }
                bytes.extend_from_slice(&encoded);
            }
        }
        Ok(bytes)
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.as_chunks::<2>().0.iter().map(|&pair| unit(pair));
    char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()
}

/// The text in `bytes` and the encoding that was found. A byte order mark
/// says; otherwise UTF-8 when it's valid, and failing that Windows-1252,
/// which every byte decodes in and encodes back to unchanged.
pub fn decode_text(bytes: &[u8]) -> (String, TextEncoding) {
    let encoding = detect(bytes);
    (encoding.decode(bytes), encoding)
}

fn detect(bytes: &[u8]) -> TextEncoding {
    if bytes.starts_with(UTF8_BOM) {
        TextEncoding::Utf8Bom
    } else if bytes.starts_with(UTF16_LE_BOM) {
        TextEncoding::Utf16Le
    } else if bytes.starts_with(UTF16_BE_BOM) {
        TextEncoding::Utf16Be
    } else if std::str::from_utf8(bytes).is_ok() {
        TextEncoding::Utf8
    } else {
        TextEncoding::Legacy(WINDOWS_1252)
    }
}

/// Whether a file starting with `head` (`TEXT_SNIFF_BYTES` of it) is text:
/// a byte order mark, valid UTF-8 (`head` may stop inside a character), or
/// an older encoding, which has no NULs and few control characters.
pub fn looks_like_text(head: &[u8]) -> bool {
    if [UTF8_BOM, UTF16_LE_BOM, UTF16_BE_BOM].iter().any(|bom| head.starts_with(bom)) {
        return true;
    }
    if head.contains(&0) {
        return false;
    }
    if std::str::from_utf8(head).map_or_else(|e| e.error_len().is_none(), |_| true) {
        return true;
    }
    let controls = head.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C | 0x1B)).count();
    controls * 50 < head.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "héllo wörld\r\nsecond\n";

    #[test]
    fn every_encoding_comes_back_as_it_went() {
        for encoding in [TextEncoding::Utf8, TextEncoding::Utf8Bom, TextEncoding::Utf16Le, TextEncoding::Utf16Be, TextEncoding::Legacy(WINDOWS_1252)] {
            let bytes = encoding.encode(TEXT).unwrap();
            assert_eq!(encoding.decode(&bytes), TEXT, "{}", encoding.label());
            assert_eq!(decode_text(&bytes), (TEXT.to_string(), encoding), "{}", encoding.label());
        }
    }

    #[test]
    fn a_byte_order_mark_is_never_text() {
        let (text, encoding) = decode_text(b"\xEF\xBB\xBFabc");
        assert_eq!((text.as_str(), encoding), ("abc", TextEncoding::Utf8Bom));
        assert_eq!(encoding.encode("Zbc").unwrap(), b"\xEF\xBB\xBFZbc");
        assert_eq!(TextEncoding::Utf16Le.encode("a").unwrap(), b"\xFF\xFEa\0");
        assert_eq!(TextEncoding::Utf16Be.encode("a").unwrap(), b"\xFE\xFF\0a");
    }

    #[test]
    fn utf16_keeps_what_needs_two_units() {
        let bytes = TextEncoding::Utf16Le.encode("a😀").unwrap();
        assert_eq!(decode_text(&bytes).0, "a😀");
    }

    #[test]
    fn an_older_encoding_holds_every_byte_it_was_given() {
        let bytes: Vec<u8> = (0x20..=0xFF).collect();
        let (text, encoding) = decode_text(&bytes);
        assert_eq!(encoding, TextEncoding::Legacy(WINDOWS_1252));
        assert_eq!(encoding.encode(&text).unwrap(), bytes);
    }

    #[test]
    fn a_character_that_doesnt_fit_is_named_not_dropped() {
        assert_eq!(TextEncoding::Legacy(WINDOWS_1252).encode("ok €"), Ok(vec![b'o', b'k', b' ', 0x80]));
        assert_eq!(TextEncoding::Legacy(WINDOWS_1252).encode("ok 😀 日"), Err('😀'));
        assert_eq!(TextEncoding::Legacy(SHIFT_JIS).encode("日本").map(|b| b.len()), Ok(4));
    }

    #[test]
    fn text_and_binary_are_told_apart() {
        assert!(looks_like_text(b"plain"));
        assert!(looks_like_text("日本語".as_bytes()));
        assert!(looks_like_text(&"日本語".as_bytes()[..4]), "cut inside a character");
        assert!(looks_like_text(b"caf\xE9 cr\xE8me\n"), "Latin-1");
        assert!(looks_like_text(b"\xFF\xFEh\0i\0"), "UTF-16 has NULs and a mark");
        assert!(!looks_like_text(b"abc\0def"));
        let noise: Vec<u8> = (1..=255).cycle().take(1000).collect();
        assert!(!looks_like_text(&noise), "control characters all through it");
    }
}
