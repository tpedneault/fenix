use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;

/// A single query capture before overlap resolution: a byte range, which
/// pattern (position in the `.scm` file) produced it, and the capture
/// name tree-sitter assigned it (e.g. `"keyword"`, `"function.method"`).
#[derive(Debug, Clone)]
pub(crate) struct RawCapture<'a> {
    pub range: Range<usize>,
    pub pattern_index: usize,
    pub name: &'a str,
}

/// Flattens a set of possibly-overlapping/nested captures into an ordered,
/// non-overlapping list of colored ranges.
///
/// The common case this has to get right is *nesting*, not just adjacent
/// overlap: a broad `@function` capture spanning a whole function
/// definition, with a narrower `@keyword` capture for just the `fn` token
/// inside it. A naive "sort by start, first one wins" sweep would let the
/// broad capture swallow the narrow one instead of the other way around.
///
/// Standard interval-stabbing approach instead: collect every distinct
/// start/end point as a breakpoint, and for each resulting sub-interval
/// pick whichever covering capture is *narrowest* (most specific) --
/// ties broken by higher `pattern_index`, matching `highlights.scm`'s own
/// convention that more specific patterns are written later in the file.
/// Adjacent sub-intervals that end up with the same winning name are
/// merged back into one output range.
///
/// A sweep: `covering` holds what has started, narrowest on top, and what
/// has ended is dropped when it reaches the top. Every capture's end is a
/// breakpoint, so a capture that reaches past `start` covers the whole
/// sub-interval. A line of minified JSON has millions of captures.
pub(crate) fn resolve_overlaps(captures: Vec<RawCapture<'_>>) -> Vec<(Range<usize>, &str)> {
    if captures.is_empty() {
        return Vec::new();
    }

    let mut breakpoints: Vec<usize> = Vec::with_capacity(captures.len() * 2);
    for c in &captures {
        breakpoints.push(c.range.start);
        breakpoints.push(c.range.end);
    }
    breakpoints.sort_unstable();
    breakpoints.dedup();

    let mut by_start: Vec<usize> = (0..captures.len()).collect();
    by_start.sort_by_key(|&i| captures[i].range.start);
    let mut started = by_start.into_iter().peekable();
    let mut covering = BinaryHeap::new();

    let mut result: Vec<(Range<usize>, &str)> = Vec::new();
    for window in breakpoints.windows(2) {
        let (start, end) = (window[0], window[1]);
        while let Some(i) = started.next_if(|&i| captures[i].range.start <= start) {
            let c = &captures[i];
            covering.push(Reverse((c.range.end - c.range.start, usize::MAX - c.pattern_index, i)));
        }
        while covering.peek().is_some_and(|&Reverse((_, _, i))| captures[i].range.end <= start) {
            covering.pop();
        }
        let Some(&Reverse((_, _, winner))) = covering.peek() else { continue };
        let winner = &captures[winner];

        match result.last_mut() {
            Some((last_range, last_name)) if *last_name == winner.name && last_range.end == start => {
                last_range.end = end;
            }
            _ => result.push((start..end, winner.name)),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap<'a>(range: Range<usize>, pattern_index: usize, name: &'a str) -> RawCapture<'a> {
        RawCapture { range, pattern_index, name }
    }

    #[test]
    fn single_capture_passes_through() {
        let out = resolve_overlaps(vec![cap(0..5, 0, "keyword")]);
        assert_eq!(out, vec![(0..5, "keyword")]);
    }

    #[test]
    fn adjacent_non_overlapping_captures_stay_separate() {
        let out = resolve_overlaps(vec![cap(0..5, 0, "keyword"), cap(5..10, 0, "string")]);
        assert_eq!(out, vec![(0..5, "keyword"), (5..10, "string")]);
    }

    #[test]
    fn narrower_nested_capture_wins_inside_a_broader_one() {
        // @function spans the whole thing; @keyword is just "fn" inside it.
        let out = resolve_overlaps(vec![cap(0..20, 0, "function"), cap(0..2, 1, "keyword")]);
        assert_eq!(out, vec![(0..2, "keyword"), (2..20, "function")]);
    }

    #[test]
    fn narrower_capture_nested_in_the_middle_splits_the_broader_one() {
        let out = resolve_overlaps(vec![cap(0..20, 0, "string"), cap(8..10, 1, "string.escape")]);
        assert_eq!(out, vec![(0..8, "string"), (8..10, "string.escape"), (10..20, "string")]);
    }

    #[test]
    fn equal_width_tie_broken_by_higher_pattern_index() {
        let out = resolve_overlaps(vec![cap(0..5, 0, "keyword"), cap(0..5, 3, "keyword.function")]);
        assert_eq!(out, vec![(0..5, "keyword.function")]);
    }

    #[test]
    fn adjacent_same_named_spans_merge_back_together() {
        // Two separate capture instances that happen to be adjacent and
        // resolve to the same winning name should read as one span, not two.
        let out = resolve_overlaps(vec![cap(0..5, 0, "comment"), cap(5..10, 0, "comment")]);
        assert_eq!(out, vec![(0..10, "comment")]);
    }

    #[test]
    fn a_gap_between_captures_stays_uncolored() {
        let out = resolve_overlaps(vec![cap(0..2, 0, "keyword"), cap(5..7, 0, "string")]);
        assert_eq!(out, vec![(0..2, "keyword"), (5..7, "string")]);
    }

    #[test]
    fn a_broad_capture_shows_again_after_a_narrow_one_ends() {
        let out = resolve_overlaps(vec![cap(0..30, 0, "a"), cap(5..20, 0, "b"), cap(8..10, 0, "c"), cap(25..26, 0, "c")]);
        assert_eq!(out, vec![(0..5, "a"), (5..8, "b"), (8..10, "c"), (10..20, "b"), (20..25, "a"), (25..26, "c"), (26..30, "a")]);
    }

    #[test]
    fn millions_of_captures_resolve_at_once() {
        // Quadratically this is 10^12 steps: the test would never end.
        let mut captures = vec![cap(0..2_000_000, 0, "broad")];
        captures.extend((0..1_000_000).map(|i| cap(i * 2..i * 2 + 1, 1, "narrow")));
        assert_eq!(resolve_overlaps(captures).len(), 2_000_000);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(resolve_overlaps(vec![]), Vec::<(Range<usize>, &str)>::new());
    }
}
