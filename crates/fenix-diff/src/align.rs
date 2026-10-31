//! Two versions of a file lined up for reading side by side: which
//! blocks of lines match, which were added, removed or changed, and
//! where each side needs blank filler rows so that matching lines sit on
//! the same screen row.
//!
//! Works on text in memory -- a buffer being edited, against a file as
//! it is at some ref -- so it can run again on every pause in typing
//! without asking git for anything.

use std::ops::Range;

use similar::{Algorithm, DiffOp, TextDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Equal,
    /// Only on the new side.
    Added,
    /// Only on the old side.
    Removed,
    /// Lines on both sides, different.
    Changed,
}

/// A run of lines: `old` and `new` are 0-based line ranges, one of them
/// empty for an addition or a removal (positioned where it would go).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    pub old: Range<usize>,
    pub new: Range<usize>,
}

/// One of the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Alignment {
    pub blocks: Vec<Block>,
    /// Filler rows each side needs: (before which line, how many). A
    /// line past the last one means "at the end".
    pub old_fillers: Vec<(usize, usize)>,
    pub new_fillers: Vec<(usize, usize)>,
}

impl Alignment {
    /// Every block that isn't equal -- what `]c` walks.
    pub fn changes(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter().filter(|b| b.kind != BlockKind::Equal)
    }

    /// The block holding `line` on `side` -- or, first, a block missing
    /// from `side` that sits just before it, so `do` on the line under a
    /// removal brings the removed lines back (as in Vim).
    pub fn block_at(&self, side: Side, line: usize) -> Option<&Block> {
        let range = |b: &Block| match side {
            Side::Old => b.old.clone(),
            Side::New => b.new.clone(),
        };
        self.blocks.iter().find(|b| range(b).is_empty() && range(b).start == line).or_else(|| self.blocks.iter().find(|b| range(b).contains(&line)))
    }

    /// The line on the other side that matches `line` on `side` -- the
    /// same offset into an equal or changed block, else the line its
    /// block sits before.
    pub fn counterpart(&self, side: Side, line: usize) -> usize {
        let Some(block) = self.blocks.iter().find(|b| match side {
            Side::Old => b.old.contains(&line),
            Side::New => b.new.contains(&line),
        }) else {
            // Past the end: the other side's end, as far past it.
            let (mine, theirs) = self.ends();
            let (mine, theirs) = if side == Side::Old { (mine, theirs) } else { (theirs, mine) };
            return theirs + line.saturating_sub(mine);
        };
        let (from, to) = match side {
            Side::Old => (&block.old, &block.new),
            Side::New => (&block.new, &block.old),
        };
        if to.is_empty() {
            return to.start;
        }
        to.start + (line - from.start).min(to.len() - 1)
    }

    /// How many lines each side has.
    fn ends(&self) -> (usize, usize) {
        self.blocks.last().map(|b| (b.old.end, b.new.end)).unwrap_or((0, 0))
    }
}

/// Splits into lines the way an editor counts them, line endings
/// dropped (so a CRLF file and its LF copy match).
fn lines(text: &str) -> Vec<&str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect()
}

/// Lines `old` and `new` up.
pub fn align(old: &str, new: &str) -> Alignment {
    let (a, b) = (lines(old), lines(new));
    let diff = TextDiff::configure().algorithm(Algorithm::Patience).diff_slices(&a, &b);
    let mut blocks: Vec<Block> = Vec::new();
    for op in diff.ops() {
        let block = match *op {
            DiffOp::Equal { old_index, new_index, len } => Block { kind: BlockKind::Equal, old: old_index..old_index + len, new: new_index..new_index + len },
            DiffOp::Delete { old_index, old_len, new_index } => Block { kind: BlockKind::Removed, old: old_index..old_index + old_len, new: new_index..new_index },
            DiffOp::Insert { old_index, new_index, new_len } => Block { kind: BlockKind::Added, old: old_index..old_index, new: new_index..new_index + new_len },
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                Block { kind: BlockKind::Changed, old: old_index..old_index + old_len, new: new_index..new_index + new_len }
            }
        };
        // A removal straight before an addition is one change.
        match blocks.last_mut() {
            Some(last) if last.kind != BlockKind::Equal && block.kind != BlockKind::Equal && last.old.end == block.old.start && last.new.end == block.new.start => {
                last.old.end = block.old.end;
                last.new.end = block.new.end;
                last.kind = if last.old.is_empty() {
                    BlockKind::Added
                } else if last.new.is_empty() {
                    BlockKind::Removed
                } else {
                    BlockKind::Changed
                };
            }
            _ => blocks.push(block),
        }
    }
    let mut alignment = Alignment { blocks, ..Default::default() };
    for block in &alignment.blocks {
        let (o, n) = (block.old.len(), block.new.len());
        if o < n {
            alignment.old_fillers.push((block.old.end, n - o));
        } else if n < o {
            alignment.new_fillers.push((block.new.end, o - n));
        }
    }
    alignment
}

/// The characters that differ between two versions of one line, as
/// char ranges into each -- word by word, so a renamed identifier is
/// marked whole rather than letter by letter.
pub fn changed_spans(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (a, b) = (tokens(old), tokens(new));
    let diff = TextDiff::configure().algorithm(Algorithm::Patience).diff_slices(&a, &b);
    let (mut olds, mut news) = (Vec::new(), Vec::new());
    let (mut at_old, mut at_new) = (0, 0);
    for change in diff.iter_all_changes() {
        let len = change.value().chars().count();
        match change.tag() {
            similar::ChangeTag::Equal => {
                at_old += len;
                at_new += len;
            }
            similar::ChangeTag::Delete => {
                push_merged(&mut olds, at_old..at_old + len);
                at_old += len;
            }
            similar::ChangeTag::Insert => {
                push_merged(&mut news, at_new..at_new + len);
                at_new += len;
            }
        }
    }
    (olds, news)
}

/// A line as words, runs of spaces and single punctuation marks.
fn tokens(line: &str) -> Vec<&str> {
    let class = |c: char| if c.is_alphanumeric() || c == '_' { 0 } else if c.is_whitespace() { 1 } else { 2 };
    let mut out = Vec::new();
    let mut start = 0;
    let mut prev: Option<u8> = None;
    for (i, c) in line.char_indices() {
        let k = class(c);
        if prev.is_some_and(|p| p != k || k == 2) {
            out.push(&line[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

fn push_merged(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(a: &Alignment) -> Vec<(BlockKind, Range<usize>, Range<usize>)> {
        a.blocks.iter().map(|b| (b.kind, b.old.clone(), b.new.clone())).collect()
    }

    #[test]
    fn identical_texts_need_no_fillers() {
        let a = align("a\nb\nc\n", "a\nb\nc\n");
        assert!(a.old_fillers.is_empty() && a.new_fillers.is_empty());
        assert_eq!(a.changes().count(), 0);
        assert_eq!(a.counterpart(Side::New, 1), 1);
    }

    #[test]
    fn an_added_block_pads_the_old_side() {
        let a = align("a\nb\nc\n", "a\nb\nX\nY\nZ\nc\n");
        assert_eq!(kinds(&a)[1], (BlockKind::Added, 2..2, 2..5));
        assert_eq!(a.old_fillers, vec![(2, 3)], "three blank rows before old line 2 (c)");
        assert!(a.new_fillers.is_empty());
        assert_eq!(a.counterpart(Side::New, 3), 2, "an added line maps to the line its block sits before");
        assert_eq!(a.counterpart(Side::New, 5), 2, "c to c");
    }

    #[test]
    fn a_removed_block_pads_the_new_side() {
        let a = align("a\nb\nc\nd\n", "a\nd\n");
        assert_eq!(kinds(&a)[1], (BlockKind::Removed, 1..3, 1..1));
        assert_eq!(a.new_fillers, vec![(1, 2)]);
        assert_eq!(a.counterpart(Side::Old, 3), 1);
        assert_eq!(a.block_at(Side::New, 1).map(|b| b.kind), Some(BlockKind::Removed), "found from the side it isn't on");
    }

    #[test]
    fn a_changed_block_pads_the_shorter_side() {
        let a = align("a\nold one\nz\n", "a\nnew one\nnew two\nz\n");
        assert_eq!(kinds(&a)[1], (BlockKind::Changed, 1..2, 1..3));
        assert_eq!(a.old_fillers, vec![(2, 1)]);
        assert_eq!(a.counterpart(Side::New, 2), 1, "past the shorter side's end, its last line");
    }

    #[test]
    fn changes_at_the_start_and_the_end() {
        let a = align("b\nc", "a\nb\nc\nd");
        assert_eq!(a.blocks.first().unwrap().kind, BlockKind::Added);
        assert_eq!(a.old_fillers.first(), Some(&(0, 1)));
        assert!(a.blocks.last().unwrap().kind != BlockKind::Equal);
    }

    #[test]
    fn an_empty_side_is_all_filler() {
        let a = align("", "a\nb\n");
        assert_eq!(a.old_fillers.iter().map(|f| f.1).sum::<usize>() + 1, lines("a\nb\n").len());
    }

    #[test]
    fn crlf_and_lf_versions_match() {
        let a = align("a\r\nb\r\n", "a\nb\n");
        assert_eq!(a.changes().count(), 0);
    }

    #[test]
    fn changed_spans_mark_whole_words() {
        let (old, new) = changed_spans("let hdr = parse(buf);", "let hdr = parse_header(buf)?;");
        let pick = |s: &str, r: &Range<usize>| s.chars().skip(r.start).take(r.len()).collect::<String>();
        assert_eq!(old.iter().map(|r| pick("let hdr = parse(buf);", r)).collect::<Vec<_>>(), vec!["parse"]);
        assert!(new.iter().any(|r| pick("let hdr = parse_header(buf)?;", r) == "parse_header"), "{new:?}");
    }
}
