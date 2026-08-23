/// Scores how well `query` fuzzy-matches `label`, or `None` if `query`
/// isn't even a subsequence of `label` (case-insensitively). Higher is
/// better. A deliberately simple greedy-leftmost subsequence scorer, not
/// a port of fzf/telescope's own algorithm -- good enough for filtering a
/// candidate list as you type, not claiming best-in-class ranking.
///
/// Scoring, on top of +1 per matched character: +3 for matching right at
/// the start of the label, +5 for each character that continues an
/// unbroken run (no gap since the previous match), +4 when a match lands
/// right after a path/word boundary (`/`, `_`, `-`, `.`, space, or a
/// lowercase-to-uppercase transition) -- so a query like `"fp"` ranks
/// `foo/**p**ath` above `foopa**p**er`.
pub fn fuzzy_match(query: &str, label: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let query_chars: Vec<char> = query.chars().collect();
    let label_chars: Vec<char> = label.chars().collect();

    let mut score: i64 = 0;
    let mut qi = 0;
    let mut prev_matched: Option<usize> = None;

    for (li, &lc) in label_chars.iter().enumerate() {
        if qi >= query_chars.len() {
            break;
        }
        if !lc.eq_ignore_ascii_case(&query_chars[qi]) {
            continue;
        }

        score += 1;
        match prev_matched {
            Some(prev) if li == prev + 1 => score += 5,
            None if li == 0 => score += 3,
            _ => {}
        }
        if li > 0 {
            let prev_char = label_chars[li - 1];
            let separator_boundary = matches!(prev_char, '/' | '_' | '-' | '.' | ' ');
            let camel_boundary = prev_char.is_lowercase() && lc.is_uppercase();
            if separator_boundary || camel_boundary {
                score += 4;
            }
        }
        prev_matched = Some(li);
        qi += 1;
    }

    (qi == query_chars.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything_with_zero_score() {
        assert_eq!(fuzzy_match("", "anything"), Some(0));
    }

    #[test]
    fn non_subsequence_does_not_match() {
        assert_eq!(fuzzy_match("xyz", "abc"), None);
        assert_eq!(fuzzy_match("cab", "abc"), None); // right chars, wrong order
    }

    #[test]
    fn out_of_order_or_missing_chars_fail() {
        assert_eq!(fuzzy_match("abcd", "abc"), None); // query longer than what's available
    }

    #[test]
    fn subsequence_match_succeeds() {
        assert!(fuzzy_match("ac", "abc").is_some());
        assert!(fuzzy_match("main", "src/main.rs").is_some());
    }

    #[test]
    fn contiguous_match_scores_higher_than_scattered() {
        let contiguous = fuzzy_match("abc", "abcxyz").unwrap();
        let scattered = fuzzy_match("abc", "axbxcx").unwrap();
        assert!(contiguous > scattered, "contiguous={contiguous} scattered={scattered}");
    }

    #[test]
    fn boundary_match_scores_higher_than_mid_word() {
        // "fp" should prefer the 'p' right after the '/' boundary.
        let boundary = fuzzy_match("fp", "foo/path").unwrap();
        let mid_word = fuzzy_match("fp", "foopxyz").unwrap();
        assert!(boundary > mid_word, "boundary={boundary} mid_word={mid_word}");
    }

    #[test]
    fn camel_case_boundary_counts_too() {
        let camel = fuzzy_match("fp", "fooPath").unwrap();
        let mid_word = fuzzy_match("fp", "foopxyz").unwrap();
        assert!(camel > mid_word, "camel={camel} mid_word={mid_word}");
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(fuzzy_match("ABC", "abc"), fuzzy_match("abc", "abc"));
    }

    #[test]
    fn match_at_the_very_start_scores_higher_than_the_same_match_later() {
        let at_start = fuzzy_match("ab", "abxyz").unwrap();
        let later = fuzzy_match("ab", "xyzabxyz").unwrap();
        assert!(at_start > later, "at_start={at_start} later={later}");
    }
}
