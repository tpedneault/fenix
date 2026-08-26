use std::path::PathBuf;

/// Where a `Row` came from -- which configured root, which table, which
/// line of that table's `.dat` file. `root_index` (not just `root_label`)
/// is what lets cross-reference lookups (a telecommand's parameters, a
/// parameter's calibration curve, ...) stay scoped to the *same* MIB root
/// a row was found in, instead of accidentally resolving a name against
/// a different root that happens to define something by the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSource {
    pub root_index: usize,
    pub root_label: String,
    pub table: String,
    pub file: PathBuf,
    /// 1-based line number within the table file.
    pub line: usize,
}

/// One parsed MIB row: a table's column names paired with this line's
/// tab-separated values, in schema order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub table: String,
    pub fields: Vec<(String, String)>,
    pub source: RowSource,
}

impl Row {
    /// The value of `field` (e.g. `"CCF_CNAME"`) on this row, or `None`
    /// if the row has no such field -- either an unknown name, or a
    /// line with fewer tab-separated values than the schema has columns
    /// (a malformed/truncated `.dat` line; see `parse::parse_table_file`).
    pub fn get(&self, field: &str) -> Option<&str> {
        self.fields.iter().find(|(name, _)| name == field).map(|(_, value)| value.as_str())
    }

    /// `get`, with surrounding whitespace trimmed and a missing field
    /// treated the same as an empty one -- MIB fields routinely carry
    /// presentation padding, and most callers want "is there a real
    /// value here" rather than "does this field exist."
    pub fn clean(&self, field: &str) -> &str {
        self.get(field).map(str::trim).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(fields: &[(&str, &str)]) -> Row {
        Row {
            table: "ccf".to_string(),
            fields: fields.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            source: RowSource {
                root_index: 0,
                root_label: "TEST".to_string(),
                table: "ccf".to_string(),
                file: PathBuf::from("ccf.dat"),
                line: 1,
            },
        }
    }

    #[test]
    fn get_returns_the_matching_field_value() {
        let r = row(&[("CCF_CNAME", "AAA001"), ("CCF_APID", "100")]);
        assert_eq!(r.get("CCF_APID"), Some("100"));
    }

    #[test]
    fn get_is_none_for_an_unknown_field() {
        let r = row(&[("CCF_CNAME", "AAA001")]);
        assert_eq!(r.get("CCF_NOPE"), None);
    }

    #[test]
    fn clean_trims_whitespace_and_treats_missing_as_empty() {
        let r = row(&[("CCF_DESCR", "  padded  ")]);
        assert_eq!(r.clean("CCF_DESCR"), "padded");
        assert_eq!(r.clean("CCF_NOPE"), "");
    }
}
