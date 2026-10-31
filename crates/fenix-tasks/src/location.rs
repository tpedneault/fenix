//! `path:line[:col]`, wherever it appears: in a compiler's output line
//! (`output::parse_line`) and in what someone types after `fenix`
//! (`fenix src/main.rs:12:3`). One scanner for both, so a Windows drive
//! letter, a colon inside a path and a missing column are handled the
//! same way everywhere; the two callers only differ in what may follow
//! the numbers -- a `: message` in output, nothing at all on a command
//! line.

use std::path::PathBuf;

/// One reading of `text` as a path followed by `:line` and maybe `:col`:
/// the path, the numbers, and whatever follows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Split<'a> {
    pub path: &'a str,
    pub line: usize,
    pub col: Option<usize>,
    pub rest: &'a str,
}

/// The first reading of `text` that `accept` takes, trying each colon
/// left to right and, at each, `:line:col` before `:line` alone. A
/// leading drive letter (`C:\...`, `C:/...`) is walked past first so its
/// colon is never taken for the line's; the path before the numbers is
/// never empty.
pub(crate) fn split<'a>(text: &'a str, accept: impl Fn(&Split<'a>) -> bool) -> Option<Split<'a>> {
    let bytes = text.as_bytes();
    let start = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        2
    } else {
        0
    };
    let mut i = start;
    while let Some(rel) = text[i..].find(':') {
        let colon = i + rel;
        i = colon + 1;
        if colon == 0 {
            continue;
        }
        let path = &text[..colon];
        let Some((line, after_line)) = take_leading_digits(&text[colon + 1..]) else {
            continue;
        };
        if let Some((col, after_col)) = after_line.strip_prefix(':').and_then(take_leading_digits) {
            let with_col = Split {
                path,
                line,
                col: Some(col),
                rest: after_col,
            };
            if accept(&with_col) {
                return Some(with_col);
            }
        }
        let without_col = Split {
            path,
            line,
            col: None,
            rest: after_line,
        };
        if accept(&without_col) {
            return Some(without_col);
        }
    }
    None
}

fn take_leading_digits(s: &str) -> Option<(usize, &str)> {
    let digit_count = s.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count == 0 {
        return None;
    }
    let (digits, rest) = s.split_at(digit_count);
    digits.parse().ok().map(|n| (n, rest))
}

/// A path with an optional position in it, both 1-based as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPosition {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Reads one command-line argument as `path`, `path:line` or
/// `path:line:col`. The numbers must end the argument, so `notes:v2.md`
/// or `C:\x.rs` stay plain paths; with no position, the whole argument
/// is the path.
///
/// A file whose real name ends in `:<digits>` (legal on Linux) reads as
/// a position here; a caller that can look at the disk should try the
/// argument as a path first, as `fenix` does.
pub fn parse_path_position(arg: &str) -> PathPosition {
    match split(arg, |s| s.rest.is_empty()) {
        Some(s) => PathPosition {
            path: PathBuf::from(s.path),
            line: Some(s.line),
            column: s.col,
        },
        None => PathPosition {
            path: PathBuf::from(arg),
            line: None,
            column: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(path: &str, line: Option<usize>, column: Option<usize>) -> PathPosition {
        PathPosition {
            path: PathBuf::from(path),
            line,
            column,
        }
    }

    #[test]
    fn a_plain_path_has_no_position() {
        assert_eq!(
            parse_path_position("src/main.rs"),
            pos("src/main.rs", None, None)
        );
        assert_eq!(parse_path_position(""), pos("", None, None));
    }

    #[test]
    fn a_line_and_a_column() {
        assert_eq!(
            parse_path_position("src/main.rs:12"),
            pos("src/main.rs", Some(12), None)
        );
        assert_eq!(
            parse_path_position("src/main.rs:12:3"),
            pos("src/main.rs", Some(12), Some(3))
        );
    }

    #[test]
    fn a_drive_letter_is_part_of_the_path() {
        assert_eq!(parse_path_position(r"C:\x.rs"), pos(r"C:\x.rs", None, None));
        assert_eq!(
            parse_path_position(r"C:\x.rs:12"),
            pos(r"C:\x.rs", Some(12), None)
        );
        assert_eq!(
            parse_path_position(r"C:\x.rs:12:3"),
            pos(r"C:\x.rs", Some(12), Some(3))
        );
        assert_eq!(
            parse_path_position("d:/work/x.rs:7:1"),
            pos("d:/work/x.rs", Some(7), Some(1))
        );
        // A drive and nothing else is a path, not a file `C` at line 12.
        assert_eq!(parse_path_position("C:12"), pos("C:12", None, None));
    }

    #[test]
    fn numbers_that_do_not_end_the_argument_are_part_of_the_path() {
        assert_eq!(
            parse_path_position("notes:2024.md"),
            pos("notes:2024.md", None, None)
        );
        assert_eq!(
            parse_path_position("/a:1b/c.rs:3"),
            pos("/a:1b/c.rs", Some(3), None)
        );
        assert_eq!(parse_path_position("x.rs:12:"), pos("x.rs:12:", None, None));
        assert_eq!(
            parse_path_position("x.rs:12:3:4"),
            pos("x.rs:12", Some(3), Some(4))
        );
    }

    #[test]
    fn a_unc_path_keeps_its_shape() {
        assert_eq!(
            parse_path_position(r"\\server\share\x.rs:9:2"),
            pos(r"\\server\share\x.rs", Some(9), Some(2))
        );
    }

    #[test]
    fn a_bare_position_with_no_path_is_a_path() {
        assert_eq!(parse_path_position(":12"), pos(":12", None, None));
    }

    #[test]
    fn non_ascii_text_around_the_numbers_is_fine() {
        assert_eq!(
            parse_path_position("é/ñ.rs:4:2"),
            pos("é/ñ.rs", Some(4), Some(2))
        );
        assert_eq!(parse_path_position("é:ñ"), pos("é:ñ", None, None));
    }
}
