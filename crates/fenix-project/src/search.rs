//! Searching files for text, in this process: ripgrep's own libraries
//! (`ignore` walks the tree as `rg` does, honouring `.gitignore`;
//! `grep-searcher` finds the lines) behind options a person sets -- match
//! case or smart-case, whole word, literal or regex, include/exclude
//! globs, hidden and ignored files. A search streams what it finds file
//! by file, can be cancelled between files, and stops at a cap. The
//! matched ranges come from the `regex` crate, which is also what
//! [`replace`] runs, so what's replaced is exactly what was shown.

use std::collections::HashMap;
use std::ops::{ControlFlow, Range};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch};
use regex::Regex;

/// How a query is read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Exact case. Off, the search is smart-case: a capital letter in the
    /// query makes it exact, else any case matches.
    pub match_case: bool,
    /// Only where the match is a whole word.
    pub whole_word: bool,
    /// The query is a regex; off, it's literal text.
    pub regex: bool,
    /// Hidden files and the ones `.gitignore` leaves out, too.
    pub hidden: bool,
    /// Globs: `*.rs` keeps only those, `!target` leaves those out.
    pub globs: Vec<String>,
    /// Lines shown around each match.
    pub context: usize,
}

/// Where a search looks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every file under these folders (as the options allow).
    Folders(Vec<PathBuf>),
    /// Just these files, whatever the ignore files say -- and these
    /// folders' files, as the options allow (an untracked folder Git
    /// counts as one change).
    Files(Vec<PathBuf>),
}

/// A query compiled: the pattern the searcher runs and the regex the
/// ranges and a replace come from -- the same pattern, so they agree.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    regex: Regex,
    literal: bool,
}

impl Pattern {
    /// `query` as `options` read it. A regex that doesn't parse says why
    /// in a line fit to show under the field.
    pub fn new(query: &str, options: &Options) -> Result<Pattern, String> {
        if query.is_empty() {
            return Err("nothing to search for".into());
        }
        let mut source = if options.regex { query.to_string() } else { regex::escape(query) };
        if options.whole_word {
            source = format!(r"\b(?:{source})\b");
        }
        let exact = options.match_case || query.chars().any(char::is_uppercase);
        if !exact {
            source = format!("(?i){source}");
        }
        let regex = Regex::new(&source).map_err(|e| short_error(&e))?;
        Ok(Pattern { source, regex, literal: !options.regex })
    }

    /// The matches in one line (no line ending), as byte ranges.
    pub fn find_in(&self, line: &str) -> Vec<Range<usize>> {
        self.regex.find_iter(line).map(|m| m.range()).filter(|r| !r.is_empty()).collect()
    }

    /// What `replacement` makes of the match at `range` in `line`:
    /// `$1`/`${name}` (and Vim's `\1`) in a regex search, the text as it
    /// is in a literal one. `preserve_case` carries the match's case over
    /// (`HELPER` -> `ASSIST`, `Helper` -> `Assist`).
    pub fn replacement_for(&self, line: &str, range: Range<usize>, replacement: &str, preserve_case: bool) -> String {
        let matched = &line[range.clone()];
        let new = if self.literal {
            replacement.to_string()
        } else {
            // Captures for the match that starts here.
            match self.regex.captures_at(line, range.start).filter(|c| c.get(0).is_some_and(|m| m.range() == range)) {
                Some(caps) => {
                    let mut out = String::new();
                    caps.expand(&vim_groups(replacement), &mut out);
                    out
                }
                None => replacement.to_string(),
            }
        };
        if preserve_case { with_case_of(matched, &new) } else { new }
    }
}

/// The first line of a regex error, which is where it says what's wrong.
fn short_error(e: &regex::Error) -> String {
    let text = e.to_string();
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    // The regex crate's message ends with an "error: ..." line.
    match lines.iter().rev().find_map(|l| l.strip_prefix("error: ")).or(lines.last().copied()) {
        Some(line) => line.to_string(),
        None => text.clone(),
    }
}

/// Vim's `\1`..`\9` and `\0` as the regex crate's `${1}`; `\\` a
/// backslash; and `$2` always group 2, even before a letter or `_`
/// (`$2_$1`), which the regex crate would read as a group named `2_`.
fn vim_groups(replacement: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            let mut digits = String::new();
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                digits.push(d);
                chars.next();
            }
            out.push_str(&format!("${{{digits}}}"));
            continue;
        }
        if c == '\\' {
            match chars.peek().copied() {
                Some(d) if d.is_ascii_digit() => {
                    chars.next();
                    out.push_str(&format!("${{{d}}}"));
                    continue;
                }
                Some('\\') => {
                    chars.next();
                    out.push('\\');
                    continue;
                }
                _ => {}
            }
        }
        out.push(c);
    }
    out
}

/// `new` in the case `matched` was written in: all capitals, a capital
/// first, or as it is.
fn with_case_of(matched: &str, new: &str) -> String {
    let letters: Vec<char> = matched.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return new.to_string();
    }
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        return new.to_uppercase();
    }
    let mut chars = matched.chars();
    if chars.next().is_some_and(char::is_uppercase) && chars.all(|c| !c.is_uppercase()) {
        let mut out = String::new();
        let mut rest = new.chars();
        if let Some(first) = rest.next() {
            out.extend(first.to_uppercase());
        }
        out.extend(rest);
        return out;
    }
    if letters.iter().all(|c| c.is_lowercase()) {
        return new.to_lowercase();
    }
    new.to_string()
}

/// One line of a result: a match, or a line of context around one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// 1-based.
    pub number: usize,
    /// Its text, without its line ending -- around the matches when the
    /// line is too long to keep whole (`cut` says what was cut).
    pub text: String,
    /// The matches in `text`, as byte ranges. Empty for context.
    pub matches: Vec<Range<usize>>,
    /// Text was cut off the (start, end) of a long line.
    pub cut: (bool, bool),
}

/// One file's results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileResult {
    pub path: PathBuf,
    pub lines: Vec<Line>,
}

impl FileResult {
    pub fn match_count(&self) -> usize {
        self.lines.iter().map(|l| l.matches.len()).sum()
    }
}

/// How a search ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub files_searched: usize,
    pub binary_skipped: usize,
    /// It stopped at the cap.
    pub truncated: bool,
    pub cancelled: bool,
}

/// A long line is kept this many bytes around its matches.
const LONG_LINE: usize = 400;
const AROUND: usize = 120;

/// Searches `scope` for `pattern`, sending each file with a match to
/// `found` as it's searched (stop by returning `Break`). `unsaved` is
/// text to search in place of a file's disk content -- an open buffer's.
/// Stops between files when `cancel` is set, and after `max_matches`.
pub fn search(
    scope: &Scope,
    pattern: &Pattern,
    options: &Options,
    unsaved: &HashMap<PathBuf, String>,
    max_matches: usize,
    cancel: &AtomicBool,
    mut found: impl FnMut(FileResult) -> ControlFlow<()>,
) -> Result<Summary, String> {
    let matcher = RegexMatcherBuilder::new().line_terminator(Some(b'\n')).build(&pattern.source).map_err(|e| e.to_string())?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .before_context(options.context)
        .after_context(options.context)
        .build();
    let mut summary = Summary::default();
    let mut total = 0usize;
    let mut each = |path: &Path, summary: &mut Summary| -> ControlFlow<()> {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            return ControlFlow::Break(());
        }
        summary.files_searched += 1;
        let result = match unsaved.get(path) {
            Some(text) => search_text(path, text, pattern, options.context),
            None => {
                let mut sink = Collect { pattern, lines: Vec::new(), binary: false };
                if searcher.search_path(&matcher, path, &mut sink).is_err() {
                    return ControlFlow::Continue(());
                }
                if sink.binary {
                    summary.binary_skipped += 1;
                    return ControlFlow::Continue(());
                }
                FileResult { path: path.to_path_buf(), lines: trim_context(sink.lines) }
            }
        };
        let count = result.match_count();
        if count == 0 {
            return ControlFlow::Continue(());
        }
        total += count;
        if found(result).is_break() {
            return ControlFlow::Break(());
        }
        if total >= max_matches {
            summary.truncated = true;
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    };
    let (files, mut folders): (Vec<PathBuf>, Vec<PathBuf>) = match scope {
        Scope::Files(paths) => paths.iter().cloned().partition(|p| !p.is_dir()),
        Scope::Folders(folders) => (Vec::new(), folders.clone()),
    };
    for path in &files {
        if each(path, &mut summary).is_break() {
            return Ok(summary);
        }
    }
    folders.retain(|f| f.is_dir());
    let Some((first, rest)) = folders.split_first() else { return Ok(summary) };
    let mut walk = ignore::WalkBuilder::new(first);
    for folder in rest {
        walk.add(folder);
    }
    walk.hidden(!options.hidden)
        .ignore(!options.hidden)
        .git_ignore(!options.hidden)
        .git_global(!options.hidden)
        .git_exclude(!options.hidden)
        .parents(true)
        .require_git(false)
        .sort_by_file_name(|a, b| a.cmp(b));
    if !options.globs.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(first);
        for glob in options.globs.iter().map(|g| g.trim()).filter(|g| !g.is_empty()) {
            overrides.add(glob).map_err(|e| format!("{glob}: {e}"))?;
        }
        walk.overrides(overrides.build().map_err(|e| e.to_string())?);
    }
    // A `.git` folder is never what anyone's searching for.
    walk.filter_entry(|entry| entry.file_name() != ".git");
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if each(entry.path(), &mut summary).is_break() {
            break;
        }
    }
    Ok(summary)
}

/// Text in memory searched as a file would be -- an open buffer's.
fn search_text(path: &Path, text: &str, pattern: &Pattern, context: usize) -> FileResult {
    let all: Vec<&str> = text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let hits: Vec<usize> = (0..all.len()).filter(|&i| !pattern.find_in(all[i]).is_empty()).collect();
    let mut keep = vec![false; all.len()];
    for &i in &hits {
        let end = (i + context).min(all.len().saturating_sub(1));
        keep[i.saturating_sub(context)..=end].fill(true);
    }
    let lines = (0..all.len()).filter(|&i| keep[i]).map(|i| line_of(i + 1, all[i], pattern)).collect();
    FileResult { path: path.to_path_buf(), lines: trim_context(lines) }
}

/// A line as a result keeps it: its matches, cut around them if it's long.
fn line_of(number: usize, text: &str, pattern: &Pattern) -> Line {
    let matches = pattern.find_in(text);
    if text.len() <= LONG_LINE {
        return Line { number, text: text.to_string(), matches, cut: (false, false) };
    }
    let (from, to) = match (matches.first(), matches.last()) {
        (Some(first), Some(last)) => (first.start.saturating_sub(AROUND), (last.end + AROUND).min(text.len())),
        _ => (0, LONG_LINE.min(text.len())),
    };
    // A line full of matches is still kept to a few windows' worth.
    let (from, to) = (floor_char(text, from), ceil_char(text, to.min(from + 4 * LONG_LINE)));
    let matches = matches.into_iter().filter(|m| m.start >= from && m.end <= to).map(|m| m.start - from..m.end - from).collect();
    Line { number, text: text[from..to].to_string(), matches, cut: (from > 0, to < text.len()) }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Context at the very end of a file is only its final empty line.
fn trim_context(mut lines: Vec<Line>) -> Vec<Line> {
    while lines.last().is_some_and(|l| l.matches.is_empty() && l.text.is_empty()) {
        lines.pop();
    }
    lines
}

/// Collects a file's lines from the searcher.
struct Collect<'a> {
    pattern: &'a Pattern,
    lines: Vec<Line>,
    binary: bool,
}

impl Collect<'_> {
    fn push(&mut self, number: Option<u64>, bytes: &[u8], is_match: bool) {
        let text = String::from_utf8_lossy(bytes);
        let text = text.strip_suffix('\n').unwrap_or(&text);
        let text = text.strip_suffix('\r').unwrap_or(text);
        let number = number.unwrap_or(0) as usize;
        let mut line = line_of(number, text, self.pattern);
        if !is_match {
            line.matches.clear();
        } else if line.matches.is_empty() {
            // The searcher's engine and the regex disagree on this line
            // (it can't happen with one pattern, but never show a match
            // that a replace wouldn't find).
            return;
        }
        if self.lines.last().is_some_and(|l| l.number == number) {
            return;
        }
        self.lines.push(line);
    }
}

impl Sink for Collect<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        // A match spanning several lines comes as one; each is its own.
        let first = m.line_number();
        for (i, line) in m.lines().enumerate() {
            self.push(first.map(|n| n + i as u64), line, true);
        }
        Ok(true)
    }

    fn context(&mut self, _: &Searcher, c: &SinkContext<'_>) -> Result<bool, Self::Error> {
        if !matches!(c.kind(), SinkContextKind::Other) {
            self.push(c.line_number(), c.bytes(), false);
        }
        Ok(true)
    }

    fn binary_data(&mut self, _: &Searcher, _: u64) -> Result<bool, Self::Error> {
        self.binary = true;
        Ok(false)
    }
}

/// Where a match is, to leave it out of a replace: its line (1-based)
/// and which match on that line it is.
pub type MatchAt = (usize, usize);

/// `text` with every match of `pattern` replaced, except those `skip`
/// names -- matched line by line, as the search was, so the numbering
/// agrees. How many were replaced.
pub fn replace(text: &str, pattern: &Pattern, replacement: &str, preserve_case: bool, skip: &dyn Fn(MatchAt) -> bool) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut count = 0;
    for (i, raw) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let (line, cr) = match raw.strip_suffix('\r') {
            Some(line) => (line, "\r"),
            None => (raw, ""),
        };
        let mut at = 0;
        for (nth, range) in pattern.find_in(line).into_iter().enumerate() {
            out.push_str(&line[at..range.start]);
            if skip((i + 1, nth)) {
                out.push_str(&line[range.clone()]);
            } else {
                out.push_str(&pattern.replacement_for(line, range.clone(), replacement, preserve_case));
                count += 1;
            }
            at = range.end;
        }
        out.push_str(&line[at..]);
        out.push_str(cr);
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn opts() -> Options {
        Options::default()
    }

    fn run(scope: &Scope, query: &str, options: &Options) -> (Vec<FileResult>, Summary) {
        let pattern = Pattern::new(query, options).unwrap();
        let mut out = Vec::new();
        let summary = search(scope, &pattern, options, &HashMap::new(), 1000, &AtomicBool::new(false), |f| {
            out.push(f);
            ControlFlow::Continue(())
        })
        .unwrap();
        (out, summary)
    }

    #[test]
    fn literal_by_default_so_regex_characters_are_just_text() {
        let dir = TempDir::new("search_literal");
        dir.write("a.rs", "foo.bar(1)\nfooXbar\n");
        let (found, _) = run(&Scope::Folders(vec![dir.path().to_path_buf()]), "foo.bar(", &opts());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].lines.len(), 1, "fooXbar isn't foo.bar(");
        assert_eq!(found[0].lines[0].matches, vec![0..8]);
        let regex = Options { regex: true, ..opts() };
        assert!(Pattern::new("foo.bar(", &regex).unwrap_err().contains("unclosed"), "a bad regex says why");
    }

    #[test]
    fn smart_case_unless_a_capital_or_match_case() {
        let p = Pattern::new("helper", &opts()).unwrap();
        assert_eq!(p.find_in("Helper helper HELPER").len(), 3);
        let p = Pattern::new("Helper", &opts()).unwrap();
        assert_eq!(p.find_in("Helper helper HELPER"), vec![0..6]);
        let p = Pattern::new("helper", &Options { match_case: true, ..opts() }).unwrap();
        assert_eq!(p.find_in("Helper helper"), vec![7..13]);
    }

    #[test]
    fn whole_word_leaves_out_longer_names() {
        let p = Pattern::new("helper", &Options { whole_word: true, ..opts() }).unwrap();
        assert_eq!(p.find_in("helper_two helper"), vec![11..17]);
    }

    #[test]
    fn ignored_and_hidden_files_are_left_out_unless_asked_for() {
        let dir = TempDir::new("search_ignore");
        dir.write(".gitignore", "target/\n");
        dir.write("target/out.txt", "needle\n");
        dir.write(".hidden/h.txt", "needle\n");
        dir.write("src/a.txt", "needle\n");
        let scope = Scope::Folders(vec![dir.path().to_path_buf()]);
        let (found, _) = run(&scope, "needle", &opts());
        assert_eq!(found.iter().map(|f| f.path.file_name().unwrap().to_str().unwrap()).collect::<Vec<_>>(), vec!["a.txt"]);
        let (found, _) = run(&scope, "needle", &Options { hidden: true, ..opts() });
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn globs_include_and_exclude() {
        let dir = TempDir::new("search_globs");
        dir.write("a.rs", "x\n");
        dir.write("b.md", "x\n");
        dir.write("gen/c.rs", "x\n");
        let scope = Scope::Folders(vec![dir.path().to_path_buf()]);
        let names = |found: Vec<FileResult>| found.iter().map(|f| f.path.strip_prefix(dir.path()).unwrap().to_string_lossy().replace('\\', "/")).collect::<Vec<_>>();
        let (found, _) = run(&scope, "x", &Options { globs: vec!["*.rs".into()], ..opts() });
        assert_eq!(names(found), vec!["a.rs", "gen/c.rs"]);
        let (found, _) = run(&scope, "x", &Options { globs: vec!["!gen".into()], ..opts() });
        assert_eq!(names(found), vec!["a.rs", "b.md"]);
    }

    #[test]
    fn unsaved_text_is_searched_in_place_of_the_disk() {
        let dir = TempDir::new("search_unsaved");
        let path = dir.write("a.txt", "old\n");
        let pattern = Pattern::new("new", &opts()).unwrap();
        let unsaved = HashMap::from([(path.clone(), "brand new\n".to_string())]);
        let mut found = Vec::new();
        search(&Scope::Files(vec![path]), &pattern, &opts(), &unsaved, 100, &AtomicBool::new(false), |f| {
            found.push(f);
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(found[0].lines[0].text, "brand new");
    }

    #[test]
    fn context_lines_come_without_matches_and_binary_files_are_skipped() {
        let dir = TempDir::new("search_context");
        dir.write("a.txt", "one\ntwo\nthree\nfour\n");
        std::fs::write(dir.path().join("b.bin"), b"thr\0ee three").unwrap();
        let (found, summary) = run(&Scope::Folders(vec![dir.path().to_path_buf()]), "three", &Options { context: 1, ..opts() });
        assert_eq!(found.len(), 1);
        let lines: Vec<(usize, bool)> = found[0].lines.iter().map(|l| (l.number, !l.matches.is_empty())).collect();
        assert_eq!(lines, vec![(2, false), (3, true), (4, false)]);
        assert_eq!(summary.binary_skipped, 1);
    }

    #[test]
    fn the_cap_and_cancelling_stop_the_search() {
        let dir = TempDir::new("search_cap");
        for i in 0..5 {
            dir.write(&format!("f{i}.txt"), "x x\n");
        }
        let scope = Scope::Folders(vec![dir.path().to_path_buf()]);
        let pattern = Pattern::new("x", &opts()).unwrap();
        let mut n = 0;
        let summary = search(&scope, &pattern, &opts(), &HashMap::new(), 3, &AtomicBool::new(false), |_| {
            n += 1;
            ControlFlow::Continue(())
        })
        .unwrap();
        assert!(summary.truncated);
        assert_eq!(n, 2, "2 files make 4 matches, past the cap of 3");
        let summary = search(&scope, &pattern, &opts(), &HashMap::new(), 100, &AtomicBool::new(true), |_| ControlFlow::Continue(())).unwrap();
        assert!(summary.cancelled && summary.files_searched == 0);
    }

    #[test]
    fn a_long_line_is_kept_around_its_match() {
        let long = format!("{}needle{}", "a".repeat(2000), "b".repeat(2000));
        let pattern = Pattern::new("needle", &opts()).unwrap();
        let line = line_of(1, &long, &pattern);
        assert!(line.cut.0 && line.cut.1);
        assert!(line.text.len() < 600);
        assert_eq!(&line.text[line.matches[0].clone()], "needle");
    }

    #[test]
    fn replace_leaves_out_what_it_is_told_and_keeps_line_endings() {
        let pattern = Pattern::new("a", &opts()).unwrap();
        let (out, n) = replace("a a\r\nb a\n", &pattern, "X", false, &|at| at == (1, 1));
        assert_eq!(out, "X a\r\nb X\n");
        assert_eq!(n, 2);
    }

    #[test]
    fn regex_groups_and_preserved_case() {
        let pattern = Pattern::new(r"(\w+)_(\w+)", &Options { regex: true, ..opts() }).unwrap();
        assert_eq!(replace("foo_bar", &pattern, "$2_$1", false, &|_| false).0, "bar_foo");
        assert_eq!(replace("foo_bar", &pattern, r"\2-\1", false, &|_| false).0, "bar-foo");
        let literal = Pattern::new("helper", &opts()).unwrap();
        assert_eq!(replace("helper Helper HELPER", &literal, "assist", true, &|_| false).0, "assist Assist ASSIST");
        assert_eq!(replace("a $1", &Pattern::new("a", &opts()).unwrap(), "$1", false, &|_| false).0, "$1 $1", "literal text is literal");
    }
}
