//! SCOS-2000 MIB lookup: parses tab-separated MIB tables (`.dat` files,
//! per any main issue of the Database Import ICD, 4.x to 7.x -- see
//! `schema`) and XTCE files (1.0 to 1.2 -- see `xtce`) from one or more
//! configured roots into an in-memory index, and
//! provides the queries `fenix-gui` needs for telecommand/TM-packet/
//! TM-parameter/calibration lookup and building a telecommand call from
//! its MIB definition (`telecommand`). `MibSet` joins a project's MIBs
//! into the five kinds of definition the MIB page lists, with what uses
//! each and what's wrong in the files; `query` searches them, `detail`
//! gathers what a definition's page shows, `types` decodes PTC/PFC.
//! Host-agnostic: no knowledge of
//! `Buffer`/rendering/pickers, the same role `fenix-completion`/`fenix-
//! project`/`fenix-git` already play for their own external data
//! sources -- `fenix-gui` wraps `Row`s into picker candidates and detail
//! views the same way it already does for `ctags::TagEntry`/
//! `GrepMatch`.

pub mod checks;
pub mod detail;
pub mod generate;
pub mod packets;
mod parse;
pub mod query;
mod row;
pub mod schema;
mod set;
pub mod telecommand;
pub mod types;
mod xtce;

use std::path::PathBuf;

pub use parse::Problem;
pub use query::Query;
pub use row::{Row, RowSource};
pub use schema::{Icd, Schema};
pub use set::{CalKind, DefRef, Entry, Kind, MibSet, Stamp};
pub use xtce::XtceOptions;

/// How a project's MIBs are read: the ICD layout of `.dat` folders, and
/// how an XTCE file says which packet is which.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct ReadOptions {
    pub schema: Schema,
    pub xtce: XtceOptions,
}

/// What one root was read as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Format {
    /// A folder of `.dat` tables, laid out as `issue` has them --
    /// `detected` when it was worked out from the files.
    Icd { issue: Icd, detected: bool },
    /// An XTCE file, of the version its namespace says when it says one.
    Xtce { version: Option<&'static str> },
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Format::Icd { issue, detected: false } => write!(f, "ICD {issue}"),
            Format::Icd { issue, detected: true } => write!(f, "ICD {issue}, detected"),
            Format::Xtce { version: Some(v) } => write!(f, "XTCE {v}"),
            Format::Xtce { version: None } => write!(f, "XTCE"),
        }
    }
}

/// Whether a root is an XTCE file rather than a folder of tables.
pub fn is_xtce_file(path: &std::path::Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml") || e.eq_ignore_ascii_case("xtce"))
}

/// One configured MIB directory -- a label (shown in candidate lists and
/// detail views) plus the directory containing its `.dat` table files.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    options: ReadOptions,
    tables: std::collections::HashMap<String, Vec<Row>>,
    problems: Vec<Problem>,
    formats: Vec<Option<Format>>,
}

impl MibIndex {
    pub fn new(roots: Vec<MibRoot>) -> Self {
        Self::with_options(roots, ReadOptions::default())
    }

    pub fn with_options(roots: Vec<MibRoot>, options: ReadOptions) -> Self {
        Self { roots, options, tables: std::collections::HashMap::new(), problems: Vec::new(), formats: Vec::new() }
    }

    pub fn options(&self) -> &ReadOptions {
        &self.options
    }

    /// What root `root` was read as; `None` before `refresh`, or when
    /// there's nothing there.
    pub fn format(&self, root: usize) -> Option<&Format> {
        self.formats.get(root).and_then(Option::as_ref)
    }

    pub fn roots(&self) -> &[MibRoot] {
        &self.roots
    }

    /// What reading the files turned up: unreadable files, extra columns.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    /// Reparses every known table (`Schema::tables`) under every
    /// configured root that exists on disk, and every XTCE file root,
    /// replacing whatever was previously loaded. Called once lazily on
    /// the first MIB command and again on a manual refresh (`SPC m r`).
    /// A folder's tables are laid out as the issue it follows has them:
    /// the one the options give it, else the one its files fit.
    pub fn refresh(&mut self) {
        self.tables.clear();
        self.problems.clear();
        self.formats.clear();
        let schema = &self.options.schema;
        for (root_index, root) in self.roots.iter().enumerate() {
            if is_xtce_file(&root.path) {
                let (tables, problems, version) = xtce::read(root_index, &root.label, &root.path, &self.options.xtce);
                // In table order, so rows come out the same way every time.
                let mut tables: Vec<(String, Vec<Row>)> = tables.into_iter().collect();
                tables.sort_by(|a, b| a.0.cmp(&b.0));
                for (table, rows) in tables {
                    self.tables.entry(table).or_default().extend(rows);
                }
                self.problems.extend(problems);
                self.formats.push(Some(Format::Xtce { version }));
                continue;
            }
            if !root.path.is_dir() {
                self.formats.push(None);
                continue;
            }
            let mut texts: Vec<(String, PathBuf, String)> = Vec::new();
            for table in schema.tables() {
                let file = root.path.join(format!("{table}.dat"));
                match parse::read_table_file(root_index, &file) {
                    Ok(Some(text)) => texts.push((table, file, text)),
                    Ok(None) => {}
                    Err(problem) => self.problems.push(problem),
                }
            }
            let (issue, detected) = match schema.icd_for(&root.label) {
                Some(issue) => (issue, false),
                None => (schema.detect(texts.iter().map(|(t, _, s)| (t.as_str(), s.as_str()))), true),
            };
            for (table, file, text) in &texts {
                let own = schema.columns.iter().any(|(t, _)| t.eq_ignore_ascii_case(table));
                let (columns, layout) = match schema.columns(issue, table) {
                    Some(c) if own => (c, "mib.columns".to_string()),
                    Some(c) => (c, format!("ICD {issue}")),
                    None => {
                        let Some(c) = schema.columns_any(table) else { continue };
                        let since = schema::since(table).map(|s| s.label()).unwrap_or("?");
                        self.problems.push(Problem { root: root_index, file: file.clone(), line: None, message: format!("{table}.dat came in with ICD {since}, after the {issue} this MIB follows; read as {since} lays it out") });
                        (c, format!("ICD {}", Icd::LATEST))
                    }
                };
                let (found, problems) = parse::parse_table_text(root_index, &root.label, table, file, &columns, &layout, text);
                self.tables.entry(table.clone()).or_default().extend(found);
                self.problems.extend(problems);
            }
            self.formats.push(Some(Format::Icd { issue, detected }));
        }
        self.tables.retain(|_, rows| !rows.is_empty());
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

    fn ccf_line(columns: usize) -> String {
        let mut v = vec!["AAA001".to_string(), "first".to_string()];
        v.resize(columns, String::new());
        v.join("\t") + "\n"
    }

    #[test]
    fn a_mib_is_read_as_the_issue_it_is_set_to() {
        let root = temp_root("issue-set");
        write(&root, "ccf", &ccf_line(21));
        let options = ReadOptions { schema: Schema { icd: Some(Icd::V5), ..Schema::default() }, ..Default::default() };
        let mut index = MibIndex::with_options(vec![root.clone()], options);
        index.refresh();
        assert_eq!(index.rows("ccf")[0].fields.len(), 19);
        assert_eq!(index.format(0), Some(&Format::Icd { issue: Icd::V5, detected: false }));
        assert!(index.problems()[0].message.contains("ICD 5.x has 19"), "{:?}", index.problems());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_mib_set_to_auto_is_read_as_the_issue_its_files_fit() {
        let root = temp_root("issue-auto");
        write(&root, "ccf", &ccf_line(17));
        write(&root, "pid", &vec!["1"; 13].join("\t"));
        let mut index = MibIndex::with_options(vec![root.clone()], ReadOptions { schema: Schema::auto(), ..Default::default() });
        index.refresh();
        assert_eq!(index.format(0), Some(&Format::Icd { issue: Icd::V4, detected: true }));
        assert_eq!(index.format(0).unwrap().to_string(), "ICD 4.x, detected");
        assert!(index.problems().is_empty(), "{:?}", index.problems());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_table_newer_than_the_issue_is_read_all_the_same_with_a_problem() {
        let root = temp_root("issue-newer");
        write(&root, "pst", "SET1\tfirst\n");
        let options = ReadOptions { schema: Schema { icd: Some(Icd::V4), ..Schema::default() }, ..Default::default() };
        let mut index = MibIndex::with_options(vec![root.clone()], options);
        index.refresh();
        assert_eq!(index.rows("pst").len(), 1);
        assert!(index.problems()[0].message.contains("came in with ICD 6.x"), "{:?}", index.problems());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn a_missions_own_table_and_columns_are_read() {
        let root = temp_root("own-columns");
        write(&root, "xyz", "A\tB\n");
        write(&root, "prv", "1\t0\t9\tmine\n");
        let schema = Schema { columns: vec![("xyz".into(), vec!["XYZ_ONE".into(), "XYZ_TWO".into()]), ("prv".into(), vec!["PRV_NUMBR".into(), "PRV_MINVAL".into(), "PRV_MAXVAL".into(), "PRV_NOTE".into()])], ..Schema::default() };
        let mut index = MibIndex::with_options(vec![root.clone()], ReadOptions { schema, ..Default::default() });
        index.refresh();
        assert_eq!(index.rows("xyz")[0].get("XYZ_TWO"), Some("B"));
        assert_eq!(index.rows("prv")[0].get("PRV_NOTE"), Some("mine"));
        assert!(index.problems().is_empty());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn each_mib_can_follow_its_own_issue() {
        let old = temp_root("per-root-old");
        let new = temp_root("per-root-new");
        write(&old, "ccf", &ccf_line(17));
        write(&new, "ccf", &ccf_line(21));
        let schema = Schema { per_root: vec![(old.label.clone(), Some(Icd::V4))], ..Schema::default() };
        let mut index = MibIndex::with_options(vec![old.clone(), new.clone()], ReadOptions { schema, ..Default::default() });
        index.refresh();
        assert_eq!(index.format(0).unwrap().to_string(), "ICD 4.x");
        assert_eq!(index.format(1).unwrap().to_string(), "ICD 7.x");
        assert!(index.problems().is_empty());
        std::fs::remove_dir_all(&old.path).ok();
        std::fs::remove_dir_all(&new.path).ok();
    }

    #[test]
    fn an_index_with_no_configured_roots_is_harmless() {
        let mut index = MibIndex::new(Vec::new());
        index.refresh();
        assert!(index.rows("ccf").is_empty());
        assert!(index.roots().is_empty());
    }
}
