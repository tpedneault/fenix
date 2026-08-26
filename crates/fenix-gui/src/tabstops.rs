//! Real tab-stop-aware text layout -- the renderer investment behind
//! Fenix's table/spreadsheet view (`App::table_views`), and a fix in its
//! own right for the far more mundane case of a literal `\t` character
//! in any ordinary file: previously every character occupied exactly
//! one fixed-width screen column everywhere in `app.rs` (`content_
//! spans`, `caret_pixel_pos`, every selection/highlight segment
//! builder), so a literal tab rendered as whatever narrow glyph the
//! font happened to give it rather than actually being expanded.
//!
//! `expand_line`'s fast path (`!line.contains('\t')`) is the identity
//! map -- every file Fenix's own Insert mode has ever produced, and the
//! overwhelming majority of real code files, take this path and are
//! completely unaffected by anything in this module.

/// How a literal `\t` expands to a visual column.
#[derive(Debug, Clone, PartialEq)]
pub enum TabStops {
    /// Real Vim's own `:set tabstop=N` behavior: every tab advances to
    /// the next multiple of `N`. Every buffer's default.
    Fixed(usize),
    /// Per-*occurrence*, not periodic: the Kth tab on any line (0-
    /// indexed) expands to reach `stops[K]` regardless of how much raw
    /// text precedes it on that particular line -- what the table
    /// view's elastic column alignment needs: every row's Kth tab
    /// reaches the same visual column even though rows differ in raw
    /// character length, which is the entire point of elastic
    /// tabstops. A tab past the end of `stops` falls back to one more
    /// fixed-width stop counted from the last known one, so a row with
    /// more tabs than any row seen while computing `stops` still
    /// renders something sane instead of collapsing to zero width.
    Custom(Vec<usize>),
}

const CUSTOM_FALLBACK_WIDTH: usize = 8;

impl TabStops {
    /// Target visual column for the `tab_index`-th tab (0-indexed) on a
    /// line, starting from `visual_col`. Always strictly greater than
    /// `visual_col` -- a tab is never zero-width, even one that's
    /// already past where it "should" land (e.g. a stale `Custom` stop
    /// after an edit shrank that column).
    fn next_stop(&self, tab_index: usize, visual_col: usize) -> usize {
        let target = match self {
            TabStops::Fixed(width) => {
                let width = (*width).max(1);
                (visual_col / width + 1) * width
            }
            TabStops::Custom(stops) => match stops.get(tab_index) {
                Some(&stop) => stop,
                None => {
                    let last = stops.last().copied().unwrap_or(visual_col);
                    let overshoot = tab_index.saturating_sub(stops.len().saturating_sub(1)).max(1);
                    last + overshoot * CUSTOM_FALLBACK_WIDTH
                }
            },
        };
        target.max(visual_col + 1)
    }
}

/// Expands every `\t` in `line` to spaces per `stops`, returning the
/// display string plus a char-column -> visual-column map (length =
/// `line.chars().count() + 1`; the trailing entry is the line's own
/// total visual width, covering an end-exclusive range at the line's
/// end). For a line with no `\t` at all, returns `(line.to_string(),
/// identity map)` without ever consulting `stops` -- the fast path
/// every ordinary line takes.
pub fn expand_line(line: &str, stops: &TabStops) -> (String, Vec<usize>) {
    if !line.contains('\t') {
        let col_map: Vec<usize> = (0..=line.chars().count()).collect();
        return (line.to_string(), col_map);
    }
    let mut display = String::with_capacity(line.len());
    let mut col_map = Vec::with_capacity(line.chars().count() + 1);
    let mut visual_col = 0usize;
    let mut tab_index = 0usize;
    for ch in line.chars() {
        col_map.push(visual_col);
        if ch == '\t' {
            let target = stops.next_stop(tab_index, visual_col);
            for _ in visual_col..target {
                display.push(' ');
            }
            visual_col = target;
            tab_index += 1;
        } else {
            display.push(ch);
            visual_col += 1;
        }
    }
    col_map.push(visual_col);
    (display, col_map)
}

/// Maps a char column on a line to its visual column via `col_map`
/// (from `expand_line`), clamping to the map's last entry for a column
/// past the line's own end -- shared by every caret/selection call site
/// so an out-of-range column (e.g. a selection extending onto a ragged
/// shorter line) degrades the same way everywhere instead of panicking.
pub fn visual_col(col_map: &[usize], char_col: usize) -> usize {
    col_map.get(char_col).copied().unwrap_or_else(|| col_map.last().copied().unwrap_or(char_col))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_line_with_no_tab_is_the_identity_map_and_unchanged_text() {
        let (display, col_map) = expand_line("hello world", &TabStops::Fixed(8));
        assert_eq!(display, "hello world");
        assert_eq!(col_map, (0..=11).collect::<Vec<_>>());
    }

    #[test]
    fn expand_line_fixed_reaches_the_next_multiple_of_the_width() {
        let (display, col_map) = expand_line("a\tb", &TabStops::Fixed(4));
        assert_eq!(display, "a   b"); // 'a' at 0, tab expands 1..4, 'b' at 4
        assert_eq!(col_map, vec![0, 1, 4, 5]);
    }

    #[test]
    fn expand_line_fixed_from_a_nonzero_starting_column_still_lands_on_a_multiple() {
        let (display, _) = expand_line("abc\td", &TabStops::Fixed(4));
        assert_eq!(display, "abc d"); // already at col 3, next multiple of 4 is 4
    }

    #[test]
    fn expand_line_custom_reaches_each_configured_stop_in_order() {
        let stops = TabStops::Custom(vec![6, 12]);
        let (display, col_map) = expand_line("ab\tcdef\tg", &stops);
        assert_eq!(display, "ab    cdef  g");
        assert_eq!(col_map, vec![0, 1, 2, 6, 7, 8, 9, 10, 12, 13]);
    }

    #[test]
    fn expand_line_custom_past_the_configured_stops_still_advances() {
        let stops = TabStops::Custom(vec![4]);
        let (display, _) = expand_line("a\tb\tc\td", &stops);
        // 1st tab has a configured stop; the 2nd/3rd don't, and fall back
        // to fixed-width stops from the last known one -- never zero-
        // width, never panics on an out-of-range index.
        assert!(display.len() > "a\tb\tc\td".len());
    }

    #[test]
    fn next_stop_never_returns_the_starting_column_even_when_already_past_target() {
        let stops = TabStops::Custom(vec![2]);
        assert_eq!(stops.next_stop(0, 5), 6);
    }

    #[test]
    fn visual_col_clamps_to_the_maps_last_entry_past_the_lines_end() {
        let col_map = vec![0, 1, 4, 5];
        assert_eq!(visual_col(&col_map, 2), 4);
        assert_eq!(visual_col(&col_map, 99), 5);
    }
}
