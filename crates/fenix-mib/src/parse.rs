use std::path::Path;

use crate::row::{Row, RowSource};
use crate::schema;

/// Parses `table`'s `.dat` file under one MIB root into `Row`s. Never
/// fails -- a missing file, a read error, or an unknown table (no
/// schema) all just yield an empty `Vec`, the same "disclosed
/// degradation, not an error state" posture `fenix-completion::ctags::run`
/// already takes for a missing external tool. Each non-empty line is
/// split on tabs and zipped against the table's schema column names --
/// a line with fewer values than the schema has columns just leaves the
/// trailing columns absent (`Row::get` reports them as `None`, same as
/// an unknown field name); a line with more values than the schema has
/// columns silently drops the extras, matching the reference elisp
/// implementation's own `cl-loop ... for value in values` zip.
///
/// A missing file stays silent -- a MIB root only has a handful of the
/// ~35 known tables, so `MibIndex::refresh` trying every table against
/// every root means most of these calls are expected to find nothing.
/// A file that *exists* but fails to read (permission denied, a lock
/// held by something else, non-UTF-8 content) is a real anomaly, not
/// the normal case -- reported loudly instead of silently degrading a
/// whole root's worth of one table to "no rows," which otherwise reads
/// identically to "this root genuinely has none."
pub fn parse_table_file(root_index: usize, root_label: &str, table: &str, file: &Path) -> Vec<Row> {
    let Some(columns) = schema::columns(table) else { return Vec::new() };
    let contents = match std::fs::read_to_string(file) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            eprintln!("fenix: couldn't read {} for MIB root {root_label} ({err})", file.display());
            return Vec::new();
        }
    };
    let mut rows = Vec::new();
    for (i, line) in contents.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let values = line.split('\t');
        let fields: Vec<(String, String)> =
            columns.iter().zip(values).map(|(name, value)| (name.to_string(), value.to_string())).collect();
        rows.push(Row {
            table: table.to_string(),
            fields,
            source: RowSource {
                root_index,
                root_label: root_label.to_string(),
                table: table.to_string(),
                file: file.to_path_buf(),
                line: i + 1,
            },
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-mib-parse-test-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_one_row_per_non_empty_line() {
        let dir = temp_dir("basic");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\t100\n2\t-10\t10\n").unwrap();

        let rows = parse_table_file(0, "TEST", "prv", &file);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get("PRV_NUMBR"), Some("1"));
        assert_eq!(rows[0].get("PRV_MINVAL"), Some("0"));
        assert_eq!(rows[0].get("PRV_MAXVAL"), Some("100"));
        assert_eq!(rows[1].get("PRV_NUMBR"), Some("2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tracks_source_file_and_1_based_line_number() {
        let dir = temp_dir("source");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\t1\n2\t0\t1\n").unwrap();

        let rows = parse_table_file(3, "MIB-D", "prv", &file);

        assert_eq!(rows[0].source.root_index, 3);
        assert_eq!(rows[0].source.root_label, "MIB-D");
        assert_eq!(rows[0].source.file, file);
        assert_eq!(rows[0].source.line, 1);
        assert_eq!(rows[1].source.line, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_line_with_fewer_values_than_schema_columns_leaves_the_rest_absent() {
        let dir = temp_dir("short-line");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\n").unwrap(); // PRV_MAXVAL never written

        let rows = parse_table_file(0, "TEST", "prv", &file);

        assert_eq!(rows[0].get("PRV_MINVAL"), Some("0"));
        assert_eq!(rows[0].get("PRV_MAXVAL"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_line_with_more_values_than_schema_columns_drops_the_extras() {
        let dir = temp_dir("long-line");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\t100\textra\tstuff\n").unwrap();

        let rows = parse_table_file(0, "TEST", "prv", &file);

        assert_eq!(rows[0].fields.len(), 3); // schema has exactly 3 columns
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn blank_lines_are_skipped() {
        let dir = temp_dir("blank-lines");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\t1\n\n2\t0\t1\n").unwrap();

        assert_eq!(parse_table_file(0, "TEST", "prv", &file).len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_yields_no_rows_not_an_error() {
        let dir = temp_dir("missing");
        let file = dir.join("prv.dat");
        assert!(parse_table_file(0, "TEST", "prv", &file).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_exists_but_is_not_valid_utf8_yields_no_rows_not_a_panic() {
        // Still degrades gracefully (never a hard error/panic for
        // callers) -- only the *diagnostic* changed, from silent to a
        // loud `eprintln!`, since this is the kind of anomaly that
        // otherwise reads identically to "this root genuinely has no
        // rows in this table."
        let dir = temp_dir("bad-utf8");
        let file = dir.join("prv.dat");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        assert!(parse_table_file(0, "TEST", "prv", &file).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_table_yields_no_rows() {
        let dir = temp_dir("unknown-table");
        let file = dir.join("nope.dat");
        std::fs::write(&file, "a\tb\tc\n").unwrap();
        assert!(parse_table_file(0, "TEST", "nope", &file).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
