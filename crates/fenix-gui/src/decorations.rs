//! An extension's decorations of a buffer's text (`decorations-set`):
//! inlays and swatches that take room in the line, hidden text drawn as a
//! replacement, tints and underlines. Held by char index, and moved with
//! the buffer's edits (`Buffer::take_anchor_edits`) until the extension
//! sets them again. A decorated line is laid out here into the text drawn
//! and a char-column -> visual-column map, the same shape tab expansion
//! gives (`tabstops::expand_line`), so the caret, selections and every
//! other overlay land where the text went.

use std::ops::Range;

use fenix_core::EditDelta;

use crate::page::{Role, Shade};
use crate::tabstops::TabStops;

#[derive(Debug, Clone, PartialEq)]
pub enum Look {
    /// Text at `start`, taking room; `after`: it stays with the character
    /// before it as you type there.
    Text { text: String, role: Role, after: bool },
    Swatch((u8, u8, u8)),
    /// `start..end`, on one line, drawn as this off the cursor's line.
    Hide(String),
    Tint(Shade),
    Underline(Role),
    /// Text at `start` with a line through it, dimmed, staying with the
    /// character before: a value that was (the debugger's Ember Trail).
    Struck(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decoration {
    /// Char indices into the buffer: a place, for text and swatches; a
    /// range for the rest.
    pub start: usize,
    pub end: usize,
    pub look: Look,
    /// What a click on it comes back to its extension as
    /// (`decorations-set-tagged`), when it's one of those.
    pub id: Option<u32>,
}

impl Decoration {
    fn is_place(&self) -> bool {
        matches!(self.look, Look::Text { .. } | Look::Swatch(_) | Look::Struck(_))
    }

    /// Text inserted right at it moves it along (the place's char follows
    /// it), unless it's text that stays with the char before.
    fn moves_with_insert(&self) -> bool {
        !matches!(self.look, Look::Text { after: true, .. } | Look::Struck(_))
    }
}

/// An extension's decoration of `buffer`, by char index; none when it
/// marks nothing (an empty range).
pub fn from_wit(buffer: &fenix_core::Buffer, d: &fenix_wasm::host::Decoration) -> Option<Decoration> {
    use fenix_wasm::host::DecorationLook as L;
    let lines = buffer.line_count();
    let line_end = |line: usize| buffer.line_start_char(line) + buffer.line_len(line);
    let at = |(line, col): (u32, u32)| {
        let line = line as usize;
        if line >= lines {
            return buffer.len_chars();
        }
        buffer.line_start_char(line) + (col as usize).min(buffer.line_len(line))
    };
    let (start, end) = (at(d.start), at(d.end));
    let look = match &d.look {
        L::Text(t) => Look::Text { text: t.text.chars().filter(|c| !c.is_control()).collect(), role: crate::code_page::role(&t.role), after: t.after },
        L::Swatch(rgb) => Look::Swatch(*rgb),
        L::Hide(with) => Look::Hide(with.chars().filter(|c| !c.is_control()).collect()),
        L::Tint(shade) => Look::Tint(crate::code_page::shade(shade)),
        L::Underline(role) => Look::Underline(crate::code_page::role(role)),
    };
    match look {
        // After the character at `start`: the place past it, on its line.
        Look::Text { after: true, .. } => {
            let p = (start + 1).min(line_end((d.start.0 as usize).min(lines.saturating_sub(1))).max(start));
            Some(Decoration { start: p, end: p, look, id: None })
        }
        Look::Text { .. } | Look::Swatch(_) => Some(Decoration { start, end: start, look, id: None }),
        Look::Hide(_) => {
            let end = end.min(line_end((d.start.0 as usize).min(lines.saturating_sub(1))).max(start));
            (end > start).then_some(Decoration { start, end, look, id: None })
        }
        _ => (end > start).then_some(Decoration { start, end, look, id: None }),
    }
}

/// How a swatch is drawn: a block of its colour, then a space.
pub const SWATCH: &str = "  ";

/// `p` moved by one edit: where it was if the edit was all after it (or
/// at it, and it stays with what's before); along by what the edit added,
/// if it was all before; to the edit's start if it took `p`'s text away.
fn moved(p: usize, d: &EditDelta, right: bool) -> usize {
    let old_end = d.start_char + d.removed.chars().count();
    if p < d.start_char || (p == d.start_char && !right && old_end == d.start_char) {
        p
    } else if p >= old_end {
        p - old_end + d.new_end_char
    } else {
        d.start_char
    }
}

/// `decorations` moved with `edits`, in order; a range left empty (its
/// text gone) goes with it.
pub fn shift(decorations: &mut Vec<Decoration>, edits: &[EditDelta]) {
    if edits.is_empty() {
        return;
    }
    decorations.retain_mut(|dec| {
        for d in edits {
            if dec.is_place() {
                let p = moved(dec.start, d, dec.moves_with_insert());
                dec.start = p;
                dec.end = p;
            } else {
                dec.start = moved(dec.start, d, true);
                dec.end = moved(dec.end, d, false).max(dec.start);
            }
        }
        dec.is_place() || dec.end > dec.start
    });
}

/// A decorated line, as it's drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineLayout {
    /// The text drawn, one char a visual column.
    pub display: String,
    /// Char column -> visual column, a column more than the line has.
    pub col_map: Vec<usize>,
    /// Text of the decorations' own (an inlay, a replacement) by byte range
    /// of `display`, and its colour.
    pub runs: Vec<(Range<usize>, Role)>,
    /// Visual columns marked: a swatch's colour, a tint, an underline.
    pub marks: Vec<(Range<usize>, Mark)>,
    /// Visual columns of a tagged decoration's, and its id: what a click
    /// there comes back as.
    pub clicks: Vec<(Range<usize>, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mark {
    Swatch((u8, u8, u8)),
    Tint(Shade),
    Underline(Role),
    /// A line through it, in this role's colour.
    Strike(Role),
}

/// Lays out the line `text`, its first char at `line_start` in the
/// buffer: tabs expanded per `stops`, and `decorations` (any that touch
/// it) drawn in. `cursor_here`: the cursor's on it, so hidden text shows
/// as it is.
pub fn layout_line(text: &str, line_start: usize, decorations: &[&Decoration], stops: &TabStops, cursor_here: bool) -> LineLayout {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let line_end = line_start + len;
    let (expanded, tab_map) = crate::tabstops::expand_line(text, stops);
    let expanded: Vec<char> = expanded.chars().collect();
    // Places by char column; after-text first, staying with the char
    // before.
    let mut places: Vec<(usize, u8, &Decoration)> = decorations
        .iter()
        .filter(|d| d.is_place() && (line_start..=line_end).contains(&d.start))
        .map(|d| (d.start - line_start, if d.moves_with_insert() { 1 } else { 0 }, *d))
        .collect();
    places.sort_by_key(|(col, order, _)| (*col, *order));
    let hidden: Vec<(Range<usize>, &str, Option<u32>)> = if cursor_here {
        Vec::new()
    } else {
        let mut h: Vec<(Range<usize>, &str, Option<u32>)> = decorations
            .iter()
            .filter_map(|d| match &d.look {
                Look::Hide(with) if (line_start..line_end).contains(&d.start) => Some((d.start - line_start..d.end.min(line_end) - line_start, with.as_str(), d.id)),
                _ => None,
            })
            .collect();
        h.sort_by_key(|(r, _, _)| r.start);
        h
    };
    let mut out = LineLayout { col_map: vec![0; len + 1], ..LineLayout::default() };
    let mut vcol = 0;
    let mut place = places.iter().peekable();
    let mut hide = hidden.iter().peekable();
    let mut col = 0;
    let push = |out: &mut LineLayout, vcol: &mut usize, s: &str, role: Option<Role>| {
        let from = out.display.len();
        out.display.push_str(s);
        if let Some(role) = role {
            out.runs.push((from..out.display.len(), role));
        }
        *vcol += s.chars().count();
    };
    while col <= len {
        while let Some((_, _, d)) = place.next_if(|(c, _, _)| *c == col) {
            let from = vcol;
            match &d.look {
                Look::Text { text, role, .. } => push(&mut out, &mut vcol, text, Some(*role)),
                Look::Struck(text) => {
                    push(&mut out, &mut vcol, text, Some(Role::Muted));
                    out.marks.push((from..vcol, Mark::Strike(Role::Muted)));
                }
                Look::Swatch(rgb) => {
                    out.marks.push((vcol..vcol + 1, Mark::Swatch(*rgb)));
                    push(&mut out, &mut vcol, SWATCH, None);
                }
                _ => {}
            }
            if let Some(id) = d.id.filter(|_| vcol > from) {
                out.clicks.push((from..vcol, id));
            }
        }
        if col == len {
            out.col_map[col] = vcol;
            break;
        }
        // A hidden range starting here: its replacement, every char of it
        // at the replacement's start.
        while hide.next_if(|(r, _, _)| r.start < col).is_some() {}
        if let Some((range, with, id)) = hide.next_if(|(r, _, _)| r.start == col) {
            let end = range.end.max(col + 1).min(len);
            for c in col..end {
                out.col_map[c] = vcol;
            }
            let from = vcol;
            push(&mut out, &mut vcol, with, Some(Role::Muted));
            if let Some(id) = id.filter(|_| vcol > from) {
                out.clicks.push((from..vcol, id));
            }
            // Inlays inside it go with it.
            while place.next_if(|(c, _, _)| *c < end).is_some() {}
            col = end;
            continue;
        }
        out.col_map[col] = vcol;
        // The char, its tab expanded.
        let from = tab_map[col];
        let to = tab_map[col + 1];
        let s: String = expanded[from..to].iter().collect();
        push(&mut out, &mut vcol, &s, None);
        col += 1;
    }
    for d in decorations {
        let mark = match &d.look {
            Look::Tint(shade) => Some(Mark::Tint(*shade)),
            Look::Underline(role) => Some(Mark::Underline(*role)),
            // Hidden text shown as it is: a click on it is still its.
            Look::Hide(_) if cursor_here && d.id.is_some() => None,
            _ => continue,
        };
        let (a, b) = (d.start.max(line_start), d.end.min(line_end));
        if a >= b {
            continue;
        }
        let (va, vb) = (out.col_map[a - line_start], out.col_map[b - line_start]);
        if va < vb {
            if let Some(mark) = mark {
                out.marks.push((va..vb, mark));
            }
            if let Some(id) = d.id {
                out.clicks.push((va..vb, id));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(at: usize, s: &str, after: bool) -> Decoration {
        Decoration { start: at, end: at, look: Look::Text { text: s.into(), role: Role::Muted, after }, id: None }
    }

    fn edit(start: usize, new_end: usize, removed: &str) -> EditDelta {
        EditDelta { start_char: start, new_end_char: new_end, removed: removed.into() }
    }

    /// An inlay takes room: what follows moves along, and the caret's
    /// column maps past it; a swatch is a coloured cell and a space.
    #[test]
    fn virtual_text_and_swatches_take_room() {
        let d = [text(3, ": i32", true), Decoration { start: 4, end: 4, look: Look::Swatch((255, 0, 0)), id: None }];
        let refs: Vec<&Decoration> = d.iter().collect();
        let l = layout_line("let #f00", 0, &refs, &TabStops::Fixed(4), false);
        assert_eq!(l.display, "let: i32   #f00");
        assert_eq!(l.col_map, [0, 1, 2, 8, 11, 12, 13, 14, 15]);
        assert_eq!(l.runs, [(3..8, Role::Muted)]);
        assert_eq!(l.marks, [(9..10, Mark::Swatch((255, 0, 0)))]);
        // On a later line, by its own start.
        let l = layout_line("x", 4, &refs, &TabStops::Fixed(4), false);
        assert_eq!(l.display, "  x");
    }

    /// Hidden text is drawn as its replacement, every char of it at its
    /// start -- as it is on the cursor's line; tints and underlines mark
    /// visual columns, tabs expanded.
    #[test]
    fn hidden_text_tints_and_underlines() {
        let d = [
            Decoration { start: 4, end: 10, look: Look::Hide("••••".into()), id: None },
            Decoration { start: 0, end: 3, look: Look::Tint(Shade::Selection), id: None },
            Decoration { start: 11, end: 12, look: Look::Underline(Role::Bad), id: None },
        ];
        let refs: Vec<&Decoration> = d.iter().collect();
        let l = layout_line("key=secret\tx", 0, &refs, &TabStops::Fixed(4), false);
        assert_eq!(l.display, "key=••••  x");
        assert_eq!(&l.col_map[4..], [4, 4, 4, 4, 4, 4, 8, 10, 11]);
        assert_eq!(l.marks, [(0..3, Mark::Tint(Shade::Selection)), (10..11, Mark::Underline(Role::Bad))]);
        let shown = layout_line("key=secret\tx", 0, &refs, &TabStops::Fixed(4), true);
        assert_eq!(shown.display, "key=secret  x", "on the cursor's line, as it is");
    }

    /// Decorations move with the edits made before them; one typed at an
    /// inlay's place goes past an after-inlay and before a before-inlay;
    /// a range whose text goes, goes.
    #[test]
    fn decorations_move_with_edits() {
        let mut d = vec![
            text(5, "a", true),
            text(5, "b", false),
            Decoration { start: 8, end: 10, look: Look::Tint(Shade::Good), id: None },
            Decoration { start: 2, end: 4, look: Look::Underline(Role::Warn), id: None },
        ];
        // "xx" typed at 5.
        shift(&mut d, &[edit(5, 7, "")]);
        assert_eq!(d.iter().map(|d| (d.start, d.end)).collect::<Vec<_>>(), [(5, 5), (7, 7), (10, 12), (2, 4)]);
        // 0..3 deleted: the underline's start went, its end moves back.
        shift(&mut d, &[edit(0, 0, "abc")]);
        assert_eq!(d.iter().map(|d| (d.start, d.end)).collect::<Vec<_>>(), [(2, 2), (4, 4), (7, 9), (0, 1)]);
        // The tint's text replaced: the new text is tinted; deleted, the
        // tint goes.
        shift(&mut d, &[edit(7, 8, "ab")]);
        assert_eq!((d[2].start, d[2].end), (7, 8));
        shift(&mut d, &[edit(7, 7, "c")]);
        assert_eq!(d.len(), 3);
        // Text typed at a range's ends isn't in it.
        let mut r = vec![Decoration { start: 2, end: 4, look: Look::Tint(Shade::Good), id: None }];
        shift(&mut r, &[edit(4, 5, ""), edit(2, 3, "")]);
        assert_eq!((r[0].start, r[0].end), (3, 5));
    }

    /// a tagged decoration's columns, where a click is its: a
    /// swatch's, an inlay's, a tint's, hidden text's.
    #[test]
    fn a_tagged_decoration_knows_its_columns() {
        let d = [
            Decoration { start: 4, end: 4, look: Look::Swatch((255, 0, 0)), id: Some(7) },
            Decoration { start: 0, end: 3, look: Look::Tint(Shade::Selection), id: Some(8) },
            Decoration { start: 8, end: 8, look: Look::Text { text: "!!".into(), role: Role::Muted, after: false }, id: None },
            Decoration { start: 11, end: 13, look: Look::Hide("*".into()), id: Some(9) },
        ];
        let refs: Vec<&Decoration> = d.iter().collect();
        let l = layout_line("let #f00 = ab", 0, &refs, &TabStops::Fixed(4), false);
        assert_eq!(l.display, "let   #f00!! = *");
        assert_eq!(l.clicks, vec![(4..6, 7), (15..16, 9), (0..3, 8)]);
        let shown = layout_line("let #f00 = ab", 0, &refs, &TabStops::Fixed(4), true);
        assert!(shown.clicks.contains(&(15..17, 9)), "{:?}", shown.clicks);
    }
}
