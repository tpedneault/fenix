//! The PDF reader's text, as the characters and boxes the worker reads
//! off a page (`fenix_pdf::text::PageChar`): which character is under a
//! point, moving a selection by words and lines, the boxes a selection
//! covers, and the text it copies. Pure; `app/reader.rs` holds the pages'
//! text and the selection.

use fenix_pdf::text::PageChar;

/// A place in a document's text: a page (from 0) and a character on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPos {
    pub page: u32,
    pub idx: usize,
}

/// How a key moves the end of a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordForward,
    WordBack,
    WordEnd,
    LineStart,
    LineEnd,
    LineDown,
    LineUp,
}

fn is_break(c: char) -> bool {
    c == '\n'
}

fn center(r: &[f32; 4]) -> (f32, f32) {
    ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0)
}

/// The character nearest `pt` (points from the page's top-left): one
/// whose box holds it, else the nearest on the nearest line.
pub fn char_at(chars: &[PageChar], pt: (f32, f32)) -> Option<usize> {
    let real = || chars.iter().enumerate().filter(|(_, c)| !is_break(c.c) && c.rect[2] > c.rect[0]);
    if let Some((i, _)) = real().find(|(_, c)| pt.0 >= c.rect[0] && pt.0 <= c.rect[2] && pt.1 >= c.rect[1] && pt.1 <= c.rect[3]) {
        return Some(i);
    }
    // Lines first: a point beside a line belongs to it, not to the line
    // below whose first character happens to be closer.
    real()
        .min_by(|(_, a), (_, b)| {
            let key = |c: &PageChar| {
                let (x, y) = center(&c.rect);
                let dy = if pt.1 < c.rect[1] { c.rect[1] - pt.1 } else if pt.1 > c.rect[3] { pt.1 - c.rect[3] } else { 0.0 };
                (dy, (x - pt.0).abs() + (y - pt.1).abs() * 0.01)
            };
            key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i)
}

/// The first character of the line nearest `y` down the page -- where
/// `v` starts a selection.
pub fn line_start_near(chars: &[PageChar], y: f32) -> Option<usize> {
    let at = char_at(chars, (0.0, y))?;
    Some(line_bounds(chars, at).0)
}

/// The line `idx` is on, as the indices of its first and last character.
fn line_bounds(chars: &[PageChar], idx: usize) -> (usize, usize) {
    let start = (0..idx).rev().find(|&i| is_break(chars[i].c)).map(|i| i + 1).unwrap_or(0);
    let end = (idx..chars.len()).find(|&i| is_break(chars[i].c)).map(|i| i.saturating_sub(1)).unwrap_or(chars.len().saturating_sub(1));
    (start, end.max(start))
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

/// Where `motion` takes `idx`, on the same page. `None` when it would
/// leave the page -- down from the last line, up from the first -- for
/// the caller to carry on to the next or previous page.
pub fn step(chars: &[PageChar], idx: usize, motion: Motion) -> Option<usize> {
    let n = chars.len();
    if n == 0 {
        return None;
    }
    let idx = idx.min(n - 1);
    match motion {
        Motion::Left => idx.checked_sub(1),
        Motion::Right => (idx + 1 < n).then_some(idx + 1),
        Motion::WordForward => {
            let here = class(chars[idx].c);
            let mut i = idx;
            while i < n && class(chars[i].c) == here && here != Class::Space {
                i += 1;
            }
            while i < n && class(chars[i].c) == Class::Space {
                i += 1;
            }
            (i < n).then_some(i)
        }
        Motion::WordEnd => {
            let mut i = idx + 1;
            while i < n && class(chars[i].c) == Class::Space {
                i += 1;
            }
            if i >= n {
                return None;
            }
            let here = class(chars[i].c);
            while i + 1 < n && class(chars[i + 1].c) == here {
                i += 1;
            }
            Some(i)
        }
        Motion::WordBack => {
            if idx == 0 {
                return None;
            }
            let mut i = idx - 1;
            while i > 0 && class(chars[i].c) == Class::Space {
                i -= 1;
            }
            let here = class(chars[i].c);
            while i > 0 && class(chars[i - 1].c) == here && here != Class::Space {
                i -= 1;
            }
            Some(i)
        }
        Motion::LineStart => Some(line_bounds(chars, idx).0),
        Motion::LineEnd => Some(line_bounds(chars, idx).1),
        Motion::LineDown | Motion::LineUp => {
            let (start, end) = line_bounds(chars, idx);
            let x = center(&chars[idx].rect).0;
            let other = if motion == Motion::LineDown {
                let next = end + 2;
                (next < n).then(|| line_bounds(chars, next))?
            } else {
                let prev = start.checked_sub(2)?;
                line_bounds(chars, prev)
            };
            (other.0..=other.1).filter(|&i| !is_break(chars[i].c)).min_by(|&a, &b| {
                (center(&chars[a].rect).0 - x).abs().partial_cmp(&(center(&chars[b].rect).0 - x).abs()).unwrap_or(std::cmp::Ordering::Equal)
            })
        }
    }
}

/// The boxes `from..=to` covers on one page, one per line.
pub fn line_rects(chars: &[PageChar], from: usize, to: usize) -> Vec<[f32; 4]> {
    let mut rects: Vec<[f32; 4]> = Vec::new();
    let mut open = false;
    for c in chars.iter().take(to.saturating_add(1)).skip(from) {
        if is_break(c.c) || c.rect[2] <= c.rect[0] {
            open = !is_break(c.c) && open;
            continue;
        }
        match rects.last_mut() {
            Some(r) if open => {
                r[0] = r[0].min(c.rect[0]);
                r[1] = r[1].min(c.rect[1]);
                r[2] = r[2].max(c.rect[2]);
                r[3] = r[3].max(c.rect[3]);
            }
            _ => {
                rects.push(c.rect);
                open = true;
            }
        }
    }
    rects
}

/// The text `from..=to` of a page, lines run together the way the page
/// meant them: a hyphen that only split a word at a line's end goes, and
/// every other line break becomes a space.
pub fn copy_text(chars: &[PageChar], from: usize, to: usize) -> String {
    let raw: Vec<char> = chars.iter().take(to.saturating_add(1)).skip(from).map(|c| c.c).collect();
    let mut out = String::new();
    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        if c == '-' && raw.get(i + 1) == Some(&'\n') && raw.get(i + 2).is_some_and(|n| n.is_lowercase()) {
            i += 2;
            continue;
        }
        if c == '\n' {
            if !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out.trim().to_string()
}

/// Labels for `n` link hints: one letter each while there are few, two
/// when there are more -- home-row letters, so they're quick to type.
pub fn hint_labels(n: usize) -> Vec<String> {
    const KEYS: &[char] = &['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];
    if n <= KEYS.len() {
        return KEYS.iter().take(n).map(|c| c.to_string()).collect();
    }
    KEYS.iter().flat_map(|a| KEYS.iter().map(move |b| format!("{a}{b}"))).take(n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Hello world\nsecond line" laid out 6 points a character, lines
    /// 10 apart.
    fn page() -> Vec<PageChar> {
        let mut chars = Vec::new();
        for (row, line) in ["Hello world", "second line"].iter().enumerate() {
            for (col, c) in line.chars().enumerate() {
                let (x, y) = (col as f32 * 6.0, row as f32 * 10.0);
                chars.push(PageChar { c, rect: [x, y, x + 6.0, y + 8.0] });
            }
            if row == 0 {
                chars.push(PageChar { c: '\n', rect: [0.0; 4] });
            }
        }
        chars
    }

    #[test]
    fn the_character_under_a_point_or_nearest_on_its_line() {
        let chars = page();
        assert_eq!(char_at(&chars, (7.0, 4.0)), Some(1), "inside the e");
        assert_eq!(char_at(&chars, (500.0, 4.0)), Some(10), "right of line 1: its last character");
        assert_eq!(char_at(&chars, (1.0, 13.0)), Some(12), "the start of line 2");
        assert_eq!(line_start_near(&chars, 14.0), Some(12));
    }

    #[test]
    fn word_and_line_motions() {
        let chars = page();
        assert_eq!(step(&chars, 0, Motion::WordForward), Some(6), "to world");
        assert_eq!(step(&chars, 6, Motion::WordForward), Some(12), "over the break to second");
        assert_eq!(step(&chars, 6, Motion::WordEnd), Some(10));
        assert_eq!(step(&chars, 12, Motion::WordBack), Some(6));
        assert_eq!(step(&chars, 8, Motion::LineStart), Some(0));
        assert_eq!(step(&chars, 2, Motion::LineEnd), Some(10));
        assert_eq!(step(&chars, 2, Motion::LineDown), Some(14), "straight down");
        assert_eq!(step(&chars, 14, Motion::LineUp), Some(2));
        assert_eq!(step(&chars, 14, Motion::LineDown), None, "off the page: the caller goes on");
        assert_eq!(step(&chars, 0, Motion::Left), None);
    }

    #[test]
    fn a_selection_covers_one_box_per_line() {
        let chars = page();
        let rects = line_rects(&chars, 6, 17);
        assert_eq!(rects, vec![[36.0, 0.0, 66.0, 8.0], [0.0, 10.0, 36.0, 18.0]]);
    }

    #[test]
    fn copying_runs_lines_together_and_mends_split_words() {
        let chars = page();
        assert_eq!(copy_text(&chars, 6, 17), "world second");
        let split: Vec<PageChar> = "infor-\nmation, Mid-\nAtlantic".chars().map(|c| PageChar { c, rect: [0.0, 0.0, 1.0, 1.0] }).collect();
        assert_eq!(copy_text(&split, 0, split.len() - 1), "information, Mid- Atlantic", "a capital after the hyphen: a real hyphen");
    }

    #[test]
    fn hint_labels_are_one_letter_then_two() {
        assert_eq!(hint_labels(3), vec!["a", "s", "d"]);
        let many = hint_labels(12);
        assert_eq!(many.len(), 12);
        assert_eq!(&many[..2], &["aa", "as"]);
    }
}
