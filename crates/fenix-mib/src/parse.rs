use std::path::{Path, PathBuf};

use crate::row::{Row, RowSource};
use crate::schema;

/// Something wrong with a MIB's files or rows, and where: shown on the
/// MIB page, in the project's MIB settings and by the project doctor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub root: usize,
    pub file: PathBuf,
    /// 1-based, when it's about one row.
    pub line: Option<usize>,
    pub message: String,
}

/// Parses `table`'s `.dat` file under one MIB root into `Row`s. A
/// missing file is normal (a MIB root has only some of the known
/// tables) and yields nothing. A file that exists but can't be read, or
/// isn't UTF-8, yields no rows and a problem. Each non-empty line is
/// split on tabs and zipped against the table's schema column names:
/// a line with fewer values leaves the trailing columns absent
/// (`Row::get` reports `None`), and one with more drops the extras,
/// with a problem saying so.
pub fn parse_table_file(root_index: usize, root_label: &str, table: &str, file: &Path) -> (Vec<Row>, Vec<Problem>) {
    let Some(columns) = schema::columns(table) else { return (Vec::new(), Vec::new()) };
    let problem = |line: Option<usize>, message: String| Problem { root: root_index, file: file.to_path_buf(), line, message };
    let contents = match std::fs::read(file) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => return (Vec::new(), vec![problem(None, "isn't UTF-8 text -- no rows read".to_string())]),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(err) => return (Vec::new(), vec![problem(None, format!("couldn't be read ({err})"))]),
    };
    let mut rows = Vec::new();
    let mut problems = Vec::new();
    for (i, line) in contents.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let count = line.split('\t').count();
        if count > columns.len() {
            problems.push(problem(Some(i + 1), format!("{count} columns, ICD 7.2 has {}; the extra ones were dropped", columns.len())));
        }
        let fields: Vec<(String, String)> =
            columns.iter().zip(line.split('\t')).map(|(name, value)| (name.to_string(), value.to_string())).collect();
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
    (rows, problems)
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

        let rows = parse_table_file(0, "TEST", "prv", &file).0;

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

        let rows = parse_table_file(3, "MIB-D", "prv", &file).0;

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

        let rows = parse_table_file(0, "TEST", "prv", &file).0;

        assert_eq!(rows[0].get("PRV_MINVAL"), Some("0"));
        assert_eq!(rows[0].get("PRV_MAXVAL"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_line_with_more_values_than_schema_columns_drops_the_extras() {
        let dir = temp_dir("long-line");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\t100\textra\tstuff\n").unwrap();

        let rows = parse_table_file(0, "TEST", "prv", &file).0;

        assert_eq!(rows[0].fields.len(), 3); // schema has exactly 3 columns
        let (_, problems) = parse_table_file(0, "TEST", "prv", &file);
        assert_eq!(problems[0].line, Some(1));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn blank_lines_are_skipped() {
        let dir = temp_dir("blank-lines");
        let file = dir.join("prv.dat");
        std::fs::write(&file, "1\t0\t1\n\n2\t0\t1\n").unwrap();

        assert_eq!(parse_table_file(0, "TEST", "prv", &file).0.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_file_yields_no_rows_not_an_error() {
        let dir = temp_dir("missing");
        let file = dir.join("prv.dat");
        assert!(parse_table_file(0, "TEST", "prv", &file).0.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_that_exists_but_is_not_valid_utf8_yields_no_rows_not_a_panic() {
        // No rows, and a problem that says why -- otherwise it reads
        // the same as "this root has no rows in this table".
        let dir = temp_dir("bad-utf8");
        let file = dir.join("prv.dat");
        std::fs::write(&file, [0xff, 0xfe, 0x00, 0x01]).unwrap();
        let (rows, problems) = parse_table_file(0, "TEST", "prv", &file);
        assert!(rows.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].message.contains("UTF-8"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unknown_table_yields_no_rows() {
        let dir = temp_dir("unknown-table");
        let file = dir.join("nope.dat");
        std::fs::write(&file, "a\tb\tc\n").unwrap();
        assert!(parse_table_file(0, "TEST", "nope", &file).0.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
