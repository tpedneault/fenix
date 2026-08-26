//! SCOS-2000 MIB lookup: parses tab-separated MIB tables (`.dat` files,
//! per the Database Import ICD 7.2 schema -- see `schema`) from one or
//! more configured root directories into an in-memory index, and
//! provides the queries `fenix-gui` needs for telecommand/TM-packet/
//! TM-parameter/calibration lookup and building a telecommand call from
//! its MIB definition (`telecommand`). Host-agnostic: no knowledge of
//! `Buffer`/rendering/pickers, the same role `fenix-completion`/`fenix-
//! project`/`fenix-git` already play for their own external data
//! sources -- `fenix-gui` wraps `Row`s into picker candidates and detail
//! views the same way it already does for `ctags::TagEntry`/
//! `GrepMatch`.

mod parse;
mod row;
pub mod schema;
pub mod telecommand;

use std::path::PathBuf;

pub use row::{Row, RowSource};

/// One configured MIB directory -- a label (shown in candidate lists and
/// detail views) plus the directory containing its `.dat` table files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MibRoot {
    pub label: String,
    pub path: PathBuf,
}

/// The parsed, queryable MIB data across every configured root. Loading
/// is eager and explicit (`refresh`), not the reference elisp
/// implementation's lazy per-file mtime cache -- simpler, and a MIB root
/// only ever has the ~35 known tables, so the cost is bounded regardless
/// of how large any individual table's row count is. Empty (no error)
/// until `refresh` is called at least once.
pub struct MibIndex {
    roots: Vec<MibRoot>,
    tables: std::collections::HashMap<String, Vec<Row>>,
}

impl MibIndex {
    pub fn new(roots: Vec<MibRoot>) -> Self {
        Self { roots, tables: std::collections::HashMap::new() }
    }

    pub fn roots(&self) -> &[MibRoot] {
        &self.roots
    }

    /// Reparses every known table (`schema::all_tables`) under every
    /// configured root that exists on disk, replacing whatever was
    /// previously loaded. Called once lazily on the first MIB command
    /// and again on a manual refresh (`SPC m r`) -- MIB data doesn't
    /// change while Fenix is running unless the user edits the raw
    /// `.dat` files elsewhere, so this isn't re-run automatically.
    pub fn refresh(&mut self) {
        self.tables.clear();
        for table in schema::all_tables() {
            let mut rows = Vec::new();
            for (root_index, root) in self.roots.iter().enumerate() {
                let file = root.path.join(format!("{table}.dat"));
                rows.extend(parse::parse_table_file(root_index, &root.label, table, &file));
            }
            if !rows.is_empty() {
                self.tables.insert(table.to_string(), rows);
            }
        }
    }

    /// Every row of `table` across every root (or an empty slice if the
    /// table has no rows loaded -- an unknown table name, or none of the
    /// configured roots have that file).
    pub fn rows(&self, table: &str) -> &[Row] {
        self.tables.get(table).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Rows of `table` whose `field` equals `value` (trimmed comparison,
    /// same as `Row::clean`), optionally scoped to one root -- `Some(i)`
    /// keeps a cross-reference lookup (a telecommand's parameters, a
    /// parameter's calibration curve, ...) within the same MIB root a
    /// starting row was found in, `None` searches every root (used for
    /// top-level candidate lists, where there's no "current root" yet).
    pub fn rows_by_field(&self, table: &str, field: &str, value: &str, root_index: Option<usize>) -> Vec<&Row> {
        let needle = value.trim();
        self.rows(table)
            .iter()
            .filter(|row| root_index.is_none_or(|i| row.source.root_index == i))
            .filter(|row| row.clean(field) == needle)
            .collect()
    }

    pub fn first_row_by_field(&self, table: &str, field: &str, value: &str, root_index: Option<usize>) -> Option<&Row> {
        self.rows_by_field(table, field, value, root_index).into_iter().next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> MibRoot {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-mib-index-test-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        MibRoot { label: name.to_uppercase(), path: dir }
    }

    fn write(root: &MibRoot, table: &str, contents: &str) {
        std::fs::write(root.path.join(format!("{table}.dat")), contents).unwrap();
    }

    #[test]
    fn refresh_loads_every_known_table_present_on_disk() {
        let root = temp_root("loads-known");
        write(&root, "ccf", "AAA001\tdo the thing\n");
        write(&root, "prv", "1\t0\t100\n");
        write(&root, "nope-unknown-table-not-written", ""); // never written, shouldn't matter

        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();

        assert_eq!(index.rows("ccf").len(), 1);
        assert_eq!(index.rows("prv").len(), 1);
        assert!(index.rows("cdf").is_empty()); // never written for this root
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn rows_by_field_finds_matching_rows_by_trimmed_value() {
        let root = temp_root("rows-by-field");
        write(&root, "ccf", "AAA001\tfirst\nAAA002\tsecond\n");
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();

        let found = index.rows_by_field("ccf", "CCF_CNAME", " AAA002 ", None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].get("CCF_DESCR"), Some("second"));
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn rows_by_field_scoped_to_a_root_ignores_a_same_named_row_in_another_root() {
        let root_a = temp_root("scope-a");
        let root_b = temp_root("scope-b");
        write(&root_a, "cpc", "SHARED_NAME\tfrom root A\n");
        write(&root_b, "cpc", "SHARED_NAME\tfrom root B\n");
        let mut index = MibIndex::new(vec![root_a.clone(), root_b.clone()]);
        index.refresh();

        let in_a = index.rows_by_field("cpc", "CPC_NAME", "SHARED_NAME", Some(0));
        assert_eq!(in_a.len(), 1);
        assert_eq!(in_a[0].get("CPC_DESCR"), Some("from root A"));

        let everywhere = index.rows_by_field("cpc", "CPC_NAME", "SHARED_NAME", None);
        assert_eq!(everywhere.len(), 2);

        std::fs::remove_dir_all(&root_a.path).ok();
        std::fs::remove_dir_all(&root_b.path).ok();
    }

    #[test]
    fn first_row_by_field_returns_none_when_nothing_matches() {
        let root = temp_root("first-none");
        write(&root, "ccf", "AAA001\tfirst\n");
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();

        assert!(index.first_row_by_field("ccf", "CCF_CNAME", "NOPE", None).is_none());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn refresh_replaces_stale_data_from_a_previous_load() {
        let root = temp_root("refresh-replaces");
        write(&root, "ccf", "AAA001\tfirst\n");
        let mut index = MibIndex::new(vec![root.clone()]);
        index.refresh();
        assert_eq!(index.rows("ccf").len(), 1);

        write(&root, "ccf", "AAA001\tfirst\nAAA002\tsecond\n");
        index.refresh();
        assert_eq!(index.rows("ccf").len(), 2);

        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_nonexistent_root_never_panics_and_yields_no_rows() {
        let mut index = MibIndex::new(vec![MibRoot { label: "GONE".to_string(), path: std::env::temp_dir().join("fenix-mib-does-not-exist") }]);
        index.refresh();
        assert!(index.rows("ccf").is_empty());
    }

    #[test]
    fn an_index_with_no_configured_roots_is_harmless() {
        let mut index = MibIndex::new(Vec::new());
        index.refresh();
        assert!(index.rows("ccf").is_empty());
        assert!(index.roots().is_empty());
    }
}
