//! What's typed after `/` on the MIB page: free words, matched against a
//! definition's name (fuzzily) and its description and other values
//! (by word), and `field:value` filters on a column -- `type:8`,
//! `apid:0x3F2`, `unit:degC`, `mib:SIM`, or any raw MIB column
//! (`ccf_critical:Y`).

use crate::set::{DefRef, Entry, Kind, MibSet};
use crate::types::parse_int;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Lower-cased.
    pub words: Vec<String>,
    /// Lower-cased keys, values as typed.
    pub filters: Vec<(String, String)>,
}

/// One match: how well it matched, and -- when a word was found only in
/// one of its other values -- which.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub def: DefRef,
    pub score: u32,
    pub matched_on: Option<String>,
}

/// The shorthand filter names, and the columns each one reads.
const FIELDS: &[(&str, &[&str])] = &[
    ("type", &["CCF_TYPE", "PID_TYPE"]),
    ("stype", &["CCF_STYPE", "PID_STYPE"]),
    ("subtype", &["CCF_STYPE", "PID_STYPE"]),
    ("apid", &["CCF_APID", "PID_APID"]),
    ("subsys", &["CCF_SUBSYS", "PCF_SUBSYS"]),
    ("unit", &["CPC_UNIT", "PCF_UNIT", "CAF_UNIT", "CCA_UNIT", "PRF_UNIT", "PID_UNIT"]),
    ("ptc", &["CPC_PTC", "PCF_PTC"]),
    ("pfc", &["CPC_PFC", "PCF_PFC"]),
    ("spid", &["PID_SPID"]),
    ("critical", &["CCF_CRITICAL"]),
];

/// The filter names `Tab` completes after `/`.
pub fn field_names() -> Vec<&'static str> {
    let mut names: Vec<&str> = FIELDS.iter().map(|(k, _)| *k).collect();
    names.extend(["mib", "cal"]);
    names
}

impl Query {
    pub fn parse(text: &str) -> Query {
        let mut q = Query::default();
        for token in text.split_whitespace() {
            match token.split_once(':') {
                Some((k, v)) if !k.is_empty() && !v.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') => {
                    q.filters.push((k.to_ascii_lowercase(), v.to_string()));
                }
                _ => q.words.push(token.to_lowercase()),
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.filters.is_empty()
    }

    /// `entry`'s values for filter `key`: the columns a shorthand reads,
    /// else a raw column by its full name (`ccf_critical`).
    fn values(set: &MibSet, entry: &Entry, key: &str) -> Vec<String> {
        match key {
            "mib" => return vec![set.root_label(entry.root).to_string()],
            "cal" => {
                return match entry.kind {
                    Kind::TcParam | Kind::TmParam => vec![entry.cols[2].clone()],
                    Kind::Calibration => vec![entry.cols[0].clone()],
                    _ => Vec::new(),
                };
            }
            _ => {}
        }
        if let Some((_, fields)) = FIELDS.iter().find(|(k, _)| *k == key) {
            return fields.iter().filter_map(|f| entry.row.get(f)).map(|v| v.trim().to_string()).collect();
        }
        entry.row.fields.iter().filter(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, v)| v.trim().to_string()).collect()
    }

    fn filter_matches(set: &MibSet, entry: &Entry, key: &str, want: &str) -> bool {
        let want_n = parse_int(want);
        Self::values(set, entry, key).iter().any(|v| match (want_n, parse_int(v)) {
            (Some(a), Some(b)) => a == b,
            _ => v.eq_ignore_ascii_case(want),
        })
    }

    /// How well `entry` matches, or `None` when it doesn't.
    pub fn score(&self, set: &MibSet, entry: &Entry) -> Option<(u32, Option<String>)> {
        if !self.filters.iter().all(|(k, v)| Self::filter_matches(set, entry, k, v)) {
            return None;
        }
        let name = entry.name.to_lowercase();
        let alias = entry.alias.to_lowercase();
        let description = entry.description.to_lowercase();
        let mut total = 1;
        let mut matched_on = None;
        for word in &self.words {
            let on_name = |n: &str| {
                if n.is_empty() {
                    0
                } else if n == word {
                    100
                } else if n.starts_with(word.as_str()) {
                    60
                } else if n.contains(word.as_str()) {
                    40
                } else if subsequence(word, n) {
                    10
                } else {
                    0
                }
            };
            let mut score = on_name(&name).max(on_name(&alias));
            if score == 0 && description.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(word.as_str())) {
                score = 20;
            } else if score == 0 && description.contains(word.as_str()) {
                score = 15;
            }
            if score == 0 {
                let extra = entry.extras.iter().find(|(_, v)| v.to_lowercase().contains(word.as_str()))?;
                score = 5;
                matched_on.get_or_insert_with(|| format!("{} {}", extra.0, extra.1));
            }
            total += score;
        }
        Some((total, matched_on))
    }

    /// Every definition of `kind` matching, best first, then by name.
    pub fn search(&self, set: &MibSet, kind: Kind) -> Vec<Hit> {
        let mut hits: Vec<Hit> = set
            .entries(kind)
            .iter()
            .enumerate()
            .filter_map(|(index, e)| self.score(set, e).map(|(score, matched_on)| Hit { def: DefRef { kind, index }, score, matched_on }))
            .collect();
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| set.get(a.def).name.cmp(&set.get(b.def).name)));
        hits
    }
}

/// Whether `needle`'s characters appear in `hay` in order.
fn subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::set::tests::fixture;

    fn names(set: &MibSet, q: &str, kind: Kind) -> Vec<String> {
        Query::parse(q).search(set, kind).into_iter().map(|h| set.get(h.def).name.clone()).collect()
    }

    #[test]
    fn words_and_filters_are_told_apart() {
        let q = Query::parse("Heater  mode type:8 apid:0x3F2 :x");
        assert_eq!(q.words, vec!["heater", "mode", ":x"]);
        assert_eq!(q.filters, vec![("type".to_string(), "8".to_string()), ("apid".to_string(), "0x3F2".to_string())]);
    }

    #[test]
    fn words_match_names_descriptions_and_other_values() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        assert_eq!(names(&set, "heater mode", Kind::TcParam), vec!["PTH00102"]);
        assert_eq!(names(&set, "pth", Kind::TcParam), vec!["PTH00101", "PTH00102"]);
        let hits = Query::parse("auto").search(&set, Kind::TcParam);
        assert_eq!(hits[0].matched_on.as_deref(), Some("status AUTO"));
        assert_eq!(names(&set, "hk_fast", Kind::TmPacket), vec!["30211"]);
        assert!(names(&set, "nothing-like-it", Kind::TmParam).is_empty());
        std::fs::remove_dir_all(&root.path).ok();
    }

    #[test]
    fn filters_compare_numbers_either_way_and_columns_by_name() {
        let root = fixture();
        let set = MibSet::load(vec![root.clone()], None);
        assert_eq!(names(&set, "apid:0x3F2", Kind::Telecommand), vec!["ZTC08101"]);
        assert_eq!(names(&set, "type:08 stype:1", Kind::Telecommand), vec!["ZTC08101"]);
        assert!(names(&set, "type:9", Kind::Telecommand).is_empty());
        assert_eq!(names(&set, "unit:DEGC", Kind::TmParam), vec!["NTH00123"]);
        assert_eq!(names(&set, "cal:status", Kind::TmParam), vec!["NTH00201"]);
        assert_eq!(names(&set, "ccf_critical:n", Kind::Telecommand), vec!["ZTC08101"]);
        assert_eq!(names(&set, "mib:fixture", Kind::TmPacket), vec!["30211"]);
        std::fs::remove_dir_all(&root.path).ok();
    }
}
