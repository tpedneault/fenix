//! Notebook search: words (every one must appear), `"quoted phrases"`,
//! and filters -- `tag:bench`, `type:note|diagram|day`,
//! `project:fenix-sat`, `links:"ADR 007"`, `after:2026-09-01`,
//! `before:...`, `is:todo`, `is:pinned`. The notebook page's filter uses
//! the same syntax.

use crate::{Entry, Kind, Notebook, Target};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Lower-cased words and phrases.
    pub terms: Vec<String>,
    pub tags: Vec<String>,
    pub kind: Option<Kind>,
    pub project: Option<String>,
    pub links: Option<String>,
    pub after: Option<chrono::NaiveDate>,
    pub before: Option<chrono::NaiveDate>,
    pub todo: bool,
    pub pinned: bool,
}

/// Splits on spaces, keeping `"quoted runs"` (and `key:"quoted"`) whole.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut q = Query::default();
        for tok in tokens(s) {
            let (key, value) = match tok.split_once(':') {
                Some((k, v)) if !v.is_empty() => (k.to_ascii_lowercase(), v.to_string()),
                _ => (String::new(), tok.clone()),
            };
            let date = || chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d").ok();
            match key.as_str() {
                "tag" => q.tags.push(value.trim_start_matches('#').to_lowercase()),
                "type" | "kind" => {
                    q.kind = match value.to_lowercase().as_str() {
                        "note" | "notes" => Some(Kind::Note),
                        "diagram" | "diagrams" => Some(Kind::Diagram),
                        "day" | "days" | "journal" => Some(Kind::Day),
                        _ => None,
                    }
                }
                "project" | "about" => q.project = Some(value.to_lowercase()),
                "links" => q.links = Some(value),
                "after" => q.after = date(),
                "before" => q.before = date(),
                "is" if value.eq_ignore_ascii_case("todo") => q.todo = true,
                "is" if value.eq_ignore_ascii_case("pinned") => q.pinned = true,
                _ => {
                    if let Some(tag) = tok.strip_prefix('#').filter(|t| !t.is_empty()) {
                        q.tags.push(tag.to_lowercase());
                    } else {
                        q.terms.push(tok.to_lowercase());
                    }
                }
            }
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }

    /// Whether `e` passes the filters (not the words).
    pub fn filters(&self, nb: &Notebook, e: &Entry) -> bool {
        if let Some(k) = self.kind {
            if e.kind != k {
                return false;
            }
        }
        for t in &self.tags {
            if !e.tags.iter().any(|x| x.to_lowercase() == *t || x.to_lowercase().starts_with(&format!("{t}/"))) {
                return false;
            }
        }
        if let Some(p) = &self.project {
            if e.about.as_deref().map(|a| a.to_lowercase() != *p).unwrap_or(true) {
                return false;
            }
        }
        if self.pinned && !e.pinned {
            return false;
        }
        if self.todo && e.open_tasks().is_empty() {
            return false;
        }
        if let Some(to) = &self.links {
            let Some(target) = nb.by_name(to) else { return false };
            if !e.links.iter().any(|l| matches!(nb.resolve(l, Some(&e.path)), Target::Entry { ref id, .. } if *id == target.id)) {
                return false;
            }
        }
        let when = e.date().or_else(|| e.modified.map(|m| chrono::DateTime::<chrono::Local>::from(m).date_naive()));
        if let (Some(after), Some(w)) = (self.after, when) {
            if w < after {
                return false;
            }
        }
        if let (Some(before), Some(w)) = (self.before, when) {
            if w >= before {
                return false;
            }
        }
        true
    }

    /// Whether every word appears in the entry's name, tags or text.
    pub fn words_match(&self, e: &Entry) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let name = e.display_name().to_lowercase();
        let text = e.text.to_lowercase();
        self.terms.iter().all(|t| name.contains(t) || text.contains(t) || e.tags.iter().any(|x| x.to_lowercase().contains(t)))
    }
}

/// One matching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub id: String,
    /// `None` when only the name (or a filter) matched.
    pub line: Option<usize>,
    pub text: String,
}

/// Every entry the query matches, most recent first, each with its
/// matching lines (up to `per_entry` of them). With `is:todo` and no
/// words, the lines are the open checkboxes.
pub fn search(nb: &Notebook, q: &Query, per_entry: usize) -> Vec<(String, Vec<Hit>)> {
    let mut out = Vec::new();
    for e in nb.recent() {
        if !q.filters(nb, e) || !q.words_match(e) {
            continue;
        }
        let mut hits = Vec::new();
        if q.todo && q.terms.is_empty() {
            for t in e.open_tasks() {
                hits.push(Hit { id: e.id.clone(), line: Some(t.line), text: e.text.lines().nth(t.line).unwrap_or("").trim().to_string() });
            }
        } else if !q.terms.is_empty() {
            for (n, line) in e.text.lines().enumerate() {
                let lower = line.to_lowercase();
                if q.terms.iter().any(|t| lower.contains(t)) {
                    if q.todo && crate::meta::task_on(line, n).map(|t| t.done).unwrap_or(true) {
                        continue;
                    }
                    hits.push(Hit { id: e.id.clone(), line: Some(n), text: line.trim().to_string() });
                    if hits.len() >= per_entry {
                        break;
                    }
                }
            }
        }
        if hits.is_empty() {
            hits.push(Hit { id: e.id.clone(), line: None, text: String::new() });
        }
        out.push((e.id.clone(), hits));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_split_words_phrases_and_filters() {
        let q = Query::parse(r#"retry "ICD says" tag:tc #bench type:diagram project:Fenix-Sat links:"ADR 007" after:2026-09-01 is:todo"#);
        assert_eq!(q.terms, vec!["retry", "icd says"]);
        assert_eq!(q.tags, vec!["tc", "bench"]);
        assert_eq!(q.kind, Some(Kind::Diagram));
        assert_eq!(q.project.as_deref(), Some("fenix-sat"));
        assert_eq!(q.links.as_deref(), Some("ADR 007"));
        assert_eq!(q.after, chrono::NaiveDate::from_ymd_opt(2026, 9, 1));
        assert!(q.todo);
        assert!(Query::parse("  ").is_empty());
    }

    #[test]
    fn searching_finds_lines_and_open_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let mut nb = Notebook::open(dir.path()).unwrap();
        let a = nb.create(Kind::Note, "Bench", "---\ntags: [bench]\n---\nretry limit is 3\n- [ ] ask about retry\n- [x] done retry\n").unwrap();
        nb.create(Kind::Note, "Other", "nothing here\n").unwrap();
        nb.create(Kind::Diagram, "Flow", "flowchart TD\n  a[retry] --> b\n").unwrap();
        let r = search(&nb, &Query::parse("retry tag:bench"), 10);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, a);
        assert_eq!(r[0].1.iter().map(|h| h.line).collect::<Vec<_>>(), vec![Some(3), Some(4), Some(5)]);
        let todo = search(&nb, &Query::parse("is:todo"), 10);
        assert_eq!(todo.len(), 1);
        assert_eq!(todo[0].1[0].text, "- [ ] ask about retry");
        let diagrams = search(&nb, &Query::parse("retry type:diagram"), 10);
        assert_eq!(diagrams.len(), 1);
        let by_todo_word = search(&nb, &Query::parse("retry is:todo"), 10);
        assert_eq!(by_todo_word[0].1.len(), 1);
    }
}
