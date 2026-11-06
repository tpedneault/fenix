//! Folds, as plain data: the ranges a buffer can fold, `(first, last)`
//! lines from 0 (the first, the header, stays on screen), and what Vim's
//! `z` commands make of them. Where the ranges come from -- the syntax
//! tree, the language server, indentation -- is `App`'s business
//! (`app/editor_ui.rs`); this is what's done with them.

use std::collections::HashSet;

pub type Range = (usize, usize);

/// The ranges sorted by their first line, outermost first, without
/// duplicates or one-line ones.
pub fn normalize(mut ranges: Vec<Range>) -> Vec<Range> {
    ranges.retain(|(start, end)| end > start);
    ranges.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    ranges.dedup();
    // One header, one fold: the outermost starting on a line.
    ranges.dedup_by_key(|r| r.0);
    ranges
}

/// Each range's depth: 0 for an outermost one, 1 inside it, ...
pub fn depths(ranges: &[Range]) -> Vec<usize> {
    let mut open: Vec<usize> = Vec::new();
    ranges
        .iter()
        .map(|&(start, end)| {
            while open.last().is_some_and(|&e| e < start) {
                open.pop();
            }
            let depth = open.len();
            open.push(end);
            depth
        })
        .collect()
}

pub fn max_depth(ranges: &[Range]) -> usize {
    depths(ranges).into_iter().max().map_or(0, |d| d + 1)
}

/// The innermost range holding `line`.
pub fn innermost(ranges: &[Range], line: usize) -> Option<Range> {
    ranges.iter().copied().filter(|&(s, e)| s <= line && line <= e).min_by_key(|&(s, e)| e - s)
}

/// The innermost closed fold holding `line`, else the innermost open
/// one: what `za` acts on.
pub fn target(ranges: &[Range], closed: &HashSet<usize>, line: usize) -> Option<Range> {
    let holding: Vec<Range> = ranges.iter().copied().filter(|&(s, e)| s <= line && line <= e).collect();
    // The outermost closed one is what's on screen.
    holding.iter().copied().filter(|r| closed.contains(&r.0)).min_by_key(|r| r.0).or_else(|| holding.into_iter().min_by_key(|&(s, e)| e - s))
}

/// The ranges inside `outer`, itself included.
pub fn within(ranges: &[Range], outer: Range) -> impl Iterator<Item = Range> + '_ {
    ranges.iter().copied().filter(move |&(s, e)| s >= outer.0 && e <= outer.1)
}

/// The headers closed at fold level `level`: those `level` deep or more
/// (`zm`, `zr`; 0 closes every one, as `zM`).
pub fn closed_at_level(ranges: &[Range], level: usize) -> HashSet<usize> {
    ranges.iter().zip(depths(ranges)).filter(|(_, d)| *d >= level).map(|(r, _)| r.0).collect()
}

/// `zj`: the first line of the next fold after `line`.
pub fn next_start(ranges: &[Range], line: usize) -> Option<usize> {
    ranges.iter().map(|r| r.0).filter(|&s| s > line).min()
}

/// `zk`: the last line of the fold before `line`.
pub fn previous_end(ranges: &[Range], line: usize) -> Option<usize> {
    ranges.iter().map(|r| r.1).filter(|&e| e < line).max()
}

/// Folds by indentation, for text no syntax tree covers: a line followed
/// by lines indented deeper (blank lines going along) folds them. `tab`
/// is a tab's width.
pub fn by_indent<'a>(lines: impl Iterator<Item = &'a str>, tab: usize) -> Vec<Range> {
    let indents: Vec<Option<usize>> = lines
        .map(|l| {
            if l.trim().is_empty() {
                return None;
            }
            Some(l.chars().take_while(|c| c.is_whitespace()).map(|c| if c == '\t' { tab.max(1) } else { 1 }).sum())
        })
        .collect();
    let mut ranges = Vec::new();
    // (header line, its indent), the ones still open.
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut last_text = 0;
    for (line, indent) in indents.iter().enumerate() {
        let Some(indent) = *indent else { continue };
        while open.last().is_some_and(|&(_, i)| indent <= i) {
            let (start, _) = open.pop().expect("just looked");
            if last_text > start {
                ranges.push((start, last_text));
            }
        }
        open.push((line, indent));
        last_text = line;
    }
    while let Some((start, _)) = open.pop() {
        if last_text > start {
            ranges.push((start, last_text));
        }
    }
    normalize(ranges)
}

/// The lines shown once `closed` headers are folded: a closed fold's
/// header stays, the rest of it goes.
pub fn shown_lines(ranges: &[Range], closed: &HashSet<usize>, line_count: usize) -> Vec<usize> {
    if closed.is_empty() {
        return (0..line_count).collect();
    }
    let ends: std::collections::HashMap<usize, usize> = ranges.iter().filter(|r| closed.contains(&r.0)).map(|&(s, e)| (s, e)).collect();
    let mut lines = Vec::new();
    let mut line = 0;
    while line < line_count {
        lines.push(line);
        line = ends.get(&line).map_or(line + 1, |end| end + 1);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = "fn a() {\n    if x {\n        y();\n    }\n}\n\nfn b() {\n    z();\n}\n";

    #[test]
    fn indentation_folds_what_is_indented_deeper() {
        let r = by_indent(RUST.lines(), 4);
        assert_eq!(r, vec![(0, 3), (1, 2), (6, 7)], "the closing brace is the next line at the same indent");
        let yaml = "a:\n  b: 1\n\n  c:\n    - d\nz: 2\n";
        assert_eq!(by_indent(yaml.lines(), 2), vec![(0, 4), (3, 4)], "blank lines go along");
    }

    #[test]
    fn levels_depths_and_targets() {
        let r = normalize(vec![(6, 8), (0, 4), (1, 3), (1, 2), (5, 5)]);
        assert_eq!(r, vec![(0, 4), (1, 3), (6, 8)]);
        assert_eq!(depths(&r), vec![0, 1, 0]);
        assert_eq!(max_depth(&r), 2);
        assert_eq!(closed_at_level(&r, 1), HashSet::from([1]));
        assert_eq!(closed_at_level(&r, 0), HashSet::from([0, 1, 6]));
        assert_eq!(innermost(&r, 2), Some((1, 3)));
        assert_eq!(target(&r, &HashSet::from([0]), 2), Some((0, 4)), "the closed one you see");
        assert_eq!(target(&r, &HashSet::new(), 2), Some((1, 3)));
        assert_eq!(within(&r, (0, 4)).count(), 2);
        assert_eq!((next_start(&r, 1), previous_end(&r, 6)), (Some(6), Some(4)));
        assert_eq!(next_start(&r, 6), None);
    }

    #[test]
    fn closed_folds_hide_all_but_their_header() {
        let r = vec![(0, 4), (1, 3), (6, 8)];
        assert_eq!(shown_lines(&r, &HashSet::from([1, 6]), 10), vec![0, 1, 4, 5, 6, 9]);
        assert_eq!(shown_lines(&r, &HashSet::from([0, 1]), 6), vec![0, 5], "a fold inside a closed one changes nothing");
    }
}
