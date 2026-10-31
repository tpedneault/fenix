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

    let mut result: Vec<(Range<usize>, &str)> = Vec::new();
    for window in breakpoints.windows(2) {
        let (start, end) = (window[0], window[1]);
        let winner = captures
            .iter()
            .filter(|c| c.range.start <= start && c.range.end >= end)
            .min_by_key(|c| (c.range.end - c.range.start, usize::MAX - c.pattern_index));
        let Some(winner) = winner else { continue };

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
    fn empty_input_yields_empty_output() {
        assert_eq!(resolve_overlaps(vec![]), Vec::<(Range<usize>, &str)>::new());
    }
}
