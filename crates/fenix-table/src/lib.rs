//! Pure, host-agnostic layout math for a delimited (currently: tab-
//! separated) table -- parsing rows and computing per-column widths and
//! visual tab-stop positions. No rendering, no buffer mutation: the
//! actual "make it look aligned" work is the renderer's own tab-stop
//! expansion (`fenix-gui`'s `tabstops` module, `TabStops::Custom`),
//! consuming the `tab_stops` this crate computes. Kept separate from
//! `fenix-gui` (like `fenix-explorer`/`fenix-project`) so it's directly
//! unit-testable without any GPU/windowing dependency, and reusable by
//! any future feature that wants "parse this delimited text and figure
//! out how wide each column needs to be," not just the table view.

/// Splits `text` into rows on `delimiter`, one row per non-blank line
/// (a line that's empty after trimming trailing `\r`/`\n` is skipped
/// entirely, rather than becoming a spurious one-field-blank row). Rows
/// are ragged where the source data is ragged -- ambiguous "how many
/// columns does this table have" cases are left to `column_widths`,
/// which already handles rows of differing lengths.
pub fn parse(text: &str, delimiter: char) -> Vec<Vec<String>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split(delimiter).map(str::to_string).collect())
        .collect()
}

/// Widest value (in chars) seen in each column, across `columns`
/// (header labels) and every row -- the number of columns is however
/// many the widest of `columns`/any row actually has; a shorter row or
/// header set just doesn't contribute to columns past its own length.
pub fn column_widths(columns: &[String], rows: &[Vec<String>]) -> Vec<usize> {
    let count = columns.len().max(rows.iter().map(Vec::len).max().unwrap_or(0));
    (0..count)
        .map(|i| {
            let header_w = columns.get(i).map(|s| s.chars().count()).unwrap_or(0);
            let data_w = rows.iter().filter_map(|r| r.get(i)).map(|s| s.chars().count()).max().unwrap_or(0);
            header_w.max(data_w)
        })
        .collect()
}

/// Cumulative visual stop position for each column: `stops[i] = sum(
/// widths[0..=i]) + (i+1)*gap` -- the visual column where column `i`'s
/// content ends and its trailing gap finishes, i.e. exactly where
/// column `i+1` starts. Consumed directly as `TabStops::Custom(stops)`
/// by the renderer: the table buffer's real content has one literal
/// `\t` after each field, and the renderer expands the line's Kth tab to
/// reach `stops[K]`.
pub fn tab_stops(widths: &[usize], gap: usize) -> Vec<usize> {
    let mut cumulative = 0usize;
    widths
        .iter()
        .map(|w| {
            cumulative += w + gap;
            cumulative
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_lines_on_the_delimiter() {
        let rows = parse("a\tb\tc\nd\te\tf\n", '\t');
        assert_eq!(rows, vec![vec!["a", "b", "c"], vec!["d", "e", "f"]]);
    }

    #[test]
    fn parse_skips_blank_lines_entirely() {
        let rows = parse("a\tb\n\nc\td\n   \n", '\t');
        assert_eq!(rows, vec![vec!["a", "b"], vec!["c", "d"]]);
    }

    #[test]
    fn parse_preserves_ragged_rows() {
        let rows = parse("a\tb\tc\nd\n", '\t');
        assert_eq!(rows, vec![vec!["a", "b", "c"], vec!["d"]]);
    }

    #[test]
    fn column_widths_takes_the_wider_of_header_or_data() {
        let columns = vec!["Name".to_string(), "X".to_string()];
        let rows = vec![vec!["Al".to_string(), "Longer".to_string()], vec!["Alexandria".to_string(), "Y".to_string()]];
        assert_eq!(column_widths(&columns, &rows), vec![10, 6]); // "Alexandria"=10, "Longer"=6
    }

    #[test]
    fn column_widths_extends_past_the_header_for_a_wider_data_row() {
        let columns = vec!["A".to_string()];
        let rows = vec![vec!["1".to_string(), "extra-column".to_string()]];
        assert_eq!(column_widths(&columns, &rows), vec![1, 12]);
    }

    #[test]
    fn column_widths_handles_ragged_rows_without_panicking() {
        let columns = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let rows = vec![vec!["x".to_string()], vec!["y".to_string(), "zz".to_string(), "zzz".to_string()]];
        assert_eq!(column_widths(&columns, &rows), vec![1, 2, 3]);
    }

    #[test]
    fn tab_stops_accumulates_widths_plus_gap_per_column() {
        assert_eq!(tab_stops(&[4, 2, 6], 1), vec![5, 8, 15]);
    }

    #[test]
    fn tab_stops_with_zero_gap_is_a_plain_running_sum() {
        assert_eq!(tab_stops(&[3, 3, 3], 0), vec![3, 6, 9]);
    }
}
