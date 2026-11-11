//! The helper's search: files walked and searched as ripgrep does it,
//! with ripgrep's own libraries, so the machine needn't have `rg`.
//!
//! It finds the lines; what in them matched, and how a long one is cut
//! for showing, Fenix works out with the same regex it shows and
//! replaces with, so the two always agree.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use grep_regex::RegexMatcherBuilder;
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch};

use crate::{Failure, FailureCode, Found, FoundLine, Search, Searched, LONGEST_LINE};

/// How long a search goes without a word before it says it's still at
/// it: well inside the time Fenix gives a machine to answer.
const QUIET: Duration = Duration::from_secs(1);

/// The most text one `Found` frame carries: a file with more comes in
/// several.
const MOST_PER_FRAME: usize = 1024 * 1024;

/// Runs `search`, handing each finding to `say`, until it's done or
/// `stop` is set.
pub fn run(search: &Search, stop: &AtomicBool, say: &mut dyn FnMut(Found)) -> Result<Searched, Failure> {
    let other = |message: String| Failure { code: FailureCode::Other, message };
    let matcher = RegexMatcherBuilder::new().line_terminator(Some(b'\n')).build(&search.pattern).map_err(|error| other(error.to_string()))?;
    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(0))
        .line_number(true)
        .before_context(search.context)
        .after_context(search.context)
        .build();
    let skip: HashSet<PathBuf> = search.skip.iter().map(PathBuf::from).collect();
    let mut searched = Searched::default();
    let mut last_word = Instant::now();

    let mut each = |path: &Path, searched: &mut Searched, say: &mut dyn FnMut(Found)| -> Result<(), Failure> {
        if stop.load(Ordering::SeqCst) {
            return Err(Failure { code: FailureCode::Cancelled, message: "stopped".into() });
        }
        if skip.contains(path) {
            return Ok(());
        }
        searched.files_searched += 1;
        let mut sink = Collect::default();
        if searcher.search_path(&matcher, path, &mut sink).is_ok() {
            if sink.binary {
                searched.binary_skipped += 1;
            } else if sink.lines.iter().any(|line| line.matched) {
                let name = path.to_string_lossy().into_owned();
                for lines in pieces(sink.lines) {
                    say(Found::File { path: name.clone(), lines });
                }
                last_word = Instant::now();
            }
        }
        if last_word.elapsed() >= QUIET {
            say(Found::Progress { searched: searched.files_searched });
            last_word = Instant::now();
        }
        Ok(())
    };

    let (files, mut folders): (Vec<PathBuf>, Vec<PathBuf>) = search.files.iter().map(PathBuf::from).partition(|path| !path.is_dir());
    folders.extend(search.folders.iter().map(PathBuf::from));
    for path in &files {
        each(path, &mut searched, say)?;
    }
    folders.retain(|folder| folder.is_dir());
    let Some((first, rest)) = folders.split_first() else { return Ok(searched) };
    let mut walk = ignore::WalkBuilder::new(first);
    for folder in rest {
        walk.add(folder);
    }
    let hidden = search.hidden;
    walk.hidden(!hidden).ignore(!hidden).git_ignore(!hidden).git_global(!hidden).git_exclude(!hidden).parents(true).require_git(false).sort_by_file_name(|a, b| a.cmp(b));
    if !search.globs.is_empty() {
        let mut overrides = ignore::overrides::OverrideBuilder::new(first);
        for glob in search.globs.iter().map(|glob| glob.trim()).filter(|glob| !glob.is_empty()) {
            overrides.add(glob).map_err(|error| other(format!("{glob}: {error}")))?;
        }
        walk.overrides(overrides.build().map_err(|error| other(error.to_string()))?);
    }
    // A `.git` folder is never what anyone's searching for.
    walk.filter_entry(|entry| entry.file_name() != ".git");
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            each(entry.path(), &mut searched, say)?;
        }
    }
    Ok(searched)
}

/// A file's lines, in frames of a size the protocol takes.
fn pieces(lines: Vec<FoundLine>) -> Vec<Vec<FoundLine>> {
    let mut pieces = vec![Vec::new()];
    let mut size = 0;
    for line in lines {
        if size + line.text.len() > MOST_PER_FRAME && !pieces.last().is_some_and(Vec::is_empty) {
            pieces.push(Vec::new());
            size = 0;
        }
        size += line.text.len() + 32;
        pieces.last_mut().expect("never empty").push(line);
    }
    pieces
}

#[derive(Default)]
struct Collect {
    lines: Vec<FoundLine>,
    binary: bool,
}

impl Collect {
    fn push(&mut self, number: Option<u64>, bytes: &[u8], matched: bool) {
        let number = number.unwrap_or(0);
        if self.lines.last().is_some_and(|line| line.number == number) {
            return;
        }
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        if text.len() > LONGEST_LINE {
            let mut end = LONGEST_LINE;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        self.lines.push(FoundLine { number, text, matched });
    }
}

impl Sink for Collect {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, found: &SinkMatch<'_>) -> Result<bool, io::Error> {
        // A match over several lines comes as one; each is its own.
        let first = found.line_number();
        for (i, line) in found.lines().enumerate() {
            self.push(first.map(|n| n + i as u64), line, true);
        }
        Ok(true)
    }

    fn context(&mut self, _: &Searcher, context: &SinkContext<'_>) -> Result<bool, io::Error> {
        if !matches!(context.kind(), SinkContextKind::Other) {
            self.push(context.line_number(), context.bytes(), false);
        }
        Ok(true)
    }

    fn binary_data(&mut self, _: &Searcher, _: u64) -> Result<bool, io::Error> {
        self.binary = true;
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(search: &Search) -> (Vec<(String, Vec<FoundLine>)>, Searched) {
        let mut files = Vec::new();
        let searched = run(search, &AtomicBool::new(false), &mut |found| {
            if let Found::File { path, lines } = found {
                files.push((path, lines));
            }
        })
        .unwrap();
        (files, searched)
    }

    #[test]
    fn a_search_finds_lines_and_leaves_out_what_git_ignores() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/a.rs"), "one\nfn helper() {}\nthree\n").unwrap();
        std::fs::write(root.join("target/built.rs"), "fn helper() {}\n").unwrap();
        std::fs::write(root.join("blob.bin"), b"helper\0\x01").unwrap();
        let folder = root.to_string_lossy().into_owned();
        let search = Search { folders: vec![folder.clone()], pattern: "helper".into(), context: 1, ..Search::default() };
        let (files, searched) = found(&search);
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(files[0].0.ends_with("a.rs"));
        let lines: Vec<(u64, &str, bool)> = files[0].1.iter().map(|line| (line.number, line.text.as_str(), line.matched)).collect();
        assert_eq!(lines, [(1, "one", false), (2, "fn helper() {}", true), (3, "three", false)]);
        assert_eq!(searched.binary_skipped, 1);

        // Hidden and ignored too, when asked; and a file left to Fenix.
        let all = Search { hidden: true, skip: vec![root.join("src/a.rs").to_string_lossy().into_owned()], ..search.clone() };
        let (files, _) = found(&all);
        assert_eq!(files.len(), 1);
        assert!(files[0].0.ends_with("built.rs"));
        // Globs keep only what they name.
        let globbed = Search { globs: vec!["*.md".into()], ..search };
        assert!(found(&globbed).0.is_empty());
    }

    #[test]
    fn a_bad_regex_says_why_and_a_stopped_search_stops() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
        let folder = dir.path().to_string_lossy().into_owned();
        assert!(run(&Search { folders: vec![folder.clone()], pattern: "(".into(), ..Search::default() }, &AtomicBool::new(false), &mut |_| {}).is_err());
        let stopped = run(&Search { folders: vec![folder], pattern: "x".into(), ..Search::default() }, &AtomicBool::new(true), &mut |_| {});
        assert!(matches!(stopped, Err(failure) if failure.code == FailureCode::Cancelled));
    }

    #[test]
    fn a_long_file_is_sent_in_pieces_and_a_long_line_cut() {
        let lines: Vec<FoundLine> = (0..40).map(|number| FoundLine { number, text: "x".repeat(60 * 1024), matched: true }).collect();
        let pieces = pieces(lines);
        assert!(pieces.len() > 1 && pieces.iter().all(|piece| !piece.is_empty()));
        assert_eq!(pieces.iter().map(Vec::len).sum::<usize>(), 40);
        let mut collect = Collect::default();
        collect.push(Some(1), "é".repeat(LONGEST_LINE).as_bytes(), true);
        assert!(collect.lines[0].text.len() <= LONGEST_LINE);
    }
}
