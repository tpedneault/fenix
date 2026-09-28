//! Telecommand calls in a script, checked against the MIB: the calls are
//! found and read back through the project's templates (the same reading
//! `SPC k e` does), and what the MIB disagrees with becomes a diagnostic
//! -- an unknown mnemonic (with the nearest known ones), a missing
//! argument, a value outside its range or status texts. A call built at
//! run time (a variable mnemonic) isn't guessed at.

use fenix_mib::telecommand;
use fenix_mib::{Kind, MibSet};

use crate::mib_form::{read_arguments, Templates};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// One finding: its line, the columns it covers, how bad, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub line: usize,
    pub cols: std::ops::Range<usize>,
    pub severity: Severity,
    pub message: String,
}

/// The template's text right before `{mnemo}`: what a call's mnemonic
/// follows. `None` when the template doesn't name the mnemonic or has
/// nothing fixed before it.
fn mnemo_marker(template: &str) -> Option<String> {
    let before = template.split("{mnemo}").next()?;
    if before.len() == template.len() {
        return None;
    }
    let marker = before.rsplit('}').next().unwrap_or(before);
    (!marker.trim().is_empty()).then(|| marker.to_string())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            cur.push((prev[j] + (ca != *cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The known telecommands closest to `name`.
fn nearest(set: &MibSet, name: &str) -> Vec<String> {
    let mut all: Vec<(usize, &str)> = set.entries(Kind::Telecommand).iter().map(|e| (levenshtein(name, &e.name), e.name.as_str())).collect();
    all.sort();
    all.into_iter().take_while(|(d, _)| *d <= 3).take(2).map(|(_, n)| n.to_string()).collect()
}

fn char_col(line: &str, byte: usize) -> usize {
    line[..byte.min(line.len())].chars().count()
}

/// Every finding in `text`.
pub fn check(text: &str, set: &MibSet, t: &Templates) -> Vec<Finding> {
    let Some(marker) = mnemo_marker(&t.command) else { return Vec::new() };
    let mut out = Vec::new();
    for (ln, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with("//") || trimmed.starts_with("--") {
            continue;
        }
        let Some(at) = line.find(&marker) else { continue };
        let start = at + marker.len();
        let word: String = line[start..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        let next = line[start + word.len()..].chars().next();
        if word.is_empty() || matches!(next, Some('$' | '[' | '(' | '{')) || line[start..].starts_with('$') {
            continue;
        }
        let cols = char_col(line, start)..char_col(line, start + word.len());
        let Some(tc) = set.resolve(&word).filter(|d| d.kind == Kind::Telecommand) else {
            let near = nearest(set, &word);
            let hint = if near.is_empty() { String::new() } else { format!(" -- did you mean {}?", near.join(" or ")) };
            out.push(Finding { line: ln, cols, severity: Severity::Error, message: format!("unknown telecommand {word}{hint}") });
            continue;
        };
        let e = set.get(tc);
        if e.row.clean("CCF_CRITICAL") == "Y" {
            out.push(Finding { line: ln, cols: cols.clone(), severity: Severity::Info, message: format!("{word} is a critical telecommand") });
        }
        let params: Vec<telecommand::TcParameter> = telecommand::tc_parameters(set.index(), &e.row).into_iter().filter(|p| !p.fixed && !p.name.is_empty()).collect();
        let names: Vec<String> = params.iter().map(|p| p.name.clone()).collect();
        let args = read_arguments(line, &names, t);
        let grouped = params.iter().any(|p| fenix_mib::types::parse_int(p.cdf.clean("CDF_GRPSIZE")).unwrap_or(0) > 0);
        for p in &params {
            let found: Vec<&(String, String)> = args.iter().filter(|(n, _)| n == &p.name).collect();
            if found.is_empty() {
                if !grouped {
                    out.push(Finding { line: ln, cols: cols.clone(), severity: Severity::Warning, message: format!("{} is missing", p.name) });
                }
                continue;
            }
            let domain = telecommand::parameter_domain(set.index(), p);
            for (_, value) in found {
                let warnings = telecommand::validate_argument(p, value, &domain);
                if let Some(w) = warnings.first() {
                    let name_at = line.find(&p.name).unwrap_or(start);
                    let vat = line[name_at..].find(value.as_str()).map(|i| name_at + i).unwrap_or(name_at);
                    out.push(Finding { line: ln, cols: char_col(line, vat)..char_col(line, vat + value.len().max(1)), severity: Severity::Warning, message: w.clone() });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn templates() -> Templates {
        Templates { command: "tc::send {mnemo} {arguments}".into(), argument: "-{name} {value}".into(), separator: ", ".into() }
    }

    #[test]
    fn unknown_mnemonics_missing_arguments_and_bad_values() {
        let root = crate::mib_page::tests::fixture();
        let set = MibSet::load(vec![root.clone()], None);
        let text = "proc checkout {} {\n    tc::send ZTC08101 -PTH00101 9, -PTH00102 AUTO\n    # tc::send NOPE\n    tc::send ZTC08109 -PTH00101 3\n    tc::send ZTC08101 -PTH00101 3\n    tc::send $cmd\n}\n";
        let found = check(text, &set, &templates());
        let msgs: Vec<(usize, &str)> = found.iter().map(|f| (f.line, f.message.as_str())).collect();
        assert!(found.iter().any(|f| f.line == 1 && f.message.contains("'9' is outside") && f.cols == (32..33)), "{msgs:?}");
        assert!(found.iter().any(|f| f.line == 3 && f.severity == Severity::Error && f.message.contains("did you mean ZTC08101")), "{msgs:?}");
        assert!(found.iter().any(|f| f.line == 4 && f.message == "PTH00102 is missing"), "{msgs:?}");
        assert!(!found.iter().any(|f| f.line == 2 || f.line == 5), "comments and variables are left alone: {msgs:?}");
        assert_eq!(mnemo_marker("telecommand_send PUS_T={type} PUS_ST={stype} APID={apid} MNEMO={mnemo} ARGUMENTS=[{arguments}]").as_deref(), Some(" MNEMO="));
        assert_eq!(mnemo_marker("{mnemo}({arguments})"), None);
        std::fs::remove_dir_all(&root.path).ok();
    }
}
