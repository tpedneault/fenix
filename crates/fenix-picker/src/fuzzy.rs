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
    let query_chars: Vec<char> = query.chars().collect();
    fuzzy_match_chars(&query_chars, label)
}

/// `fuzzy_match` with the query already split into chars -- for a caller
/// scoring one query against many labels, which then allocates nothing
/// per label.
pub fn fuzzy_match_chars(query: &[char], label: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }

    let mut score: i64 = 0;
    let mut qi = 0;
    let mut prev_matched: Option<usize> = None;
    let mut prev_char: Option<char> = None;

    for (li, lc) in label.chars().enumerate() {
        if qi >= query.len() {
            break;
        }
        let before = prev_char.replace(lc);
        if !lc.eq_ignore_ascii_case(&query[qi]) {
            continue;
        }

        score += 1;
        match prev_matched {
            Some(prev) if li == prev + 1 => score += 5,
            None if li == 0 => score += 3,
            _ => {}
        }
        if let Some(before) = before {
            let separator_boundary = matches!(before, '/' | '_' | '-' | '.' | ' ');
            let camel_boundary = before.is_lowercase() && lc.is_uppercase();
            if separator_boundary || camel_boundary {
                score += 4;
            }
        }
        prev_matched = Some(li);
        qi += 1;
    }

    (qi == query.len()).then_some(score)
}

/// Which characters of `label` `query` matched, as `fuzzy_match` scores
/// them (greedy, leftmost): char indices, for drawing them in the accent.
/// `None` when it doesn't match.
pub fn fuzzy_positions(query: &str, label: &str) -> Option<Vec<usize>> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = Vec::with_capacity(query.len());
    let mut qi = 0;
    for (li, lc) in label.chars().enumerate() {
        if qi >= query.len() {
            break;
        }
        if lc.eq_ignore_ascii_case(&query[qi]) {
            out.push(li);
            qi += 1;
        }
    }
    (qi == query.len()).then_some(out)
}

/// Where each word of `label` starts (char indices): its first character,
/// and every one after something that isn't a letter or digit, or a
/// lowercase-to-uppercase step.
fn word_starts(label: &[char]) -> Vec<usize> {
    (0..label.len())
        .filter(|&i| {
            let c = label[i];
            c.is_alphanumeric()
                && match i.checked_sub(1).map(|p| label[p]) {
                    None => true,
                    Some(before) => !before.is_alphanumeric() || (before.is_lowercase() && c.is_uppercase()),
                }
        })
        .collect()
}

/// Whether `token` (lowercase) starts at `at` in `label`, case-insensitively.
fn starts_at(label: &[char], at: usize, token: &[char]) -> bool {
    at + token.len() <= label.len() && token.iter().enumerate().all(|(i, t)| label[at + i].to_lowercase().eq(t.to_lowercase()))
}

/// How well `query` matches `label` word by word, for a list of commands
/// or names rather than paths: each of its words has to start a word of
/// `label` (strongest, and more so the earlier that word), or appear in
/// it; or, failing that, start a word of `keywords` (a description,
/// which a scattered match across would only make noise of). A one-word
/// query that does none of those still matches as `fuzzy_match` does when
/// its letters mostly run together (`gst` for "Git › Status") -- not when
/// they're scattered across the whole label. `None` when it doesn't match.
pub fn word_match(query: &str, label: &str, keywords: &str) -> Option<i64> {
    let label_chars: Vec<char> = label.chars().collect();
    let starts = word_starts(&label_chars);
    let keyword_chars: Vec<char> = keywords.chars().collect();
    let keyword_starts = word_starts(&keyword_chars);
    let tokens: Vec<Vec<char>> = query.split_whitespace().map(|t| t.chars().collect()).collect();
    if tokens.is_empty() {
        return Some(0);
    }
    let mut score = 0;
    let mut all = true;
    for token in &tokens {
        if let Some(n) = starts.iter().position(|&s| starts_at(&label_chars, s, token)) {
            score += 100 + token.len() as i64 * 5 - (n as i64).min(10) * 4;
        } else if (0..label_chars.len()).any(|i| starts_at(&label_chars, i, token)) {
            score += 40;
        } else if keyword_starts.iter().any(|&s| starts_at(&keyword_chars, s, token)) {
            score += 10;
        } else {
            all = false;
            break;
        }
    }
    if all {
        return Some(score);
    }
    let compact: String = query.split_whitespace().collect();
    let fuzzy = fuzzy_match(&compact, label)?;
    (tokens.len() == 1 && fuzzy >= compact.chars().count() as i64 * 5).then_some(fuzzy)
}

/// The characters of `label` `word_match` matched `query` on: each word
/// of the query where it starts a word of `label`, else where it first
/// appears, else (the fuzzy fallback) `fuzzy_positions`. Empty when
/// the match was in the keywords alone.
pub fn word_positions(query: &str, label: &str) -> Vec<usize> {
    let label_chars: Vec<char> = label.chars().collect();
    let starts = word_starts(&label_chars);
    let mut out = Vec::new();
    for token in query.split_whitespace() {
        let token: Vec<char> = token.chars().collect();
        let at = starts.iter().copied().find(|&s| starts_at(&label_chars, s, &token)).or_else(|| (0..label_chars.len()).find(|&i| starts_at(&label_chars, i, &token)));
        match at {
            Some(at) => out.extend(at..at + token.len()),
            None => {
                let compact: String = query.split_whitespace().collect();
                if let Some(p) = fuzzy_positions(&compact, label).filter(|_| query.split_whitespace().count() == 1) {
                    return p;
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_the_characters_matched() {
        assert_eq!(fuzzy_positions("fra", "src/frames.rs"), Some(vec![4, 5, 6]));
        assert_eq!(fuzzy_positions("xz", "abc"), None);
        assert_eq!(word_positions("git", "Git › Status page"), vec![0, 1, 2]);
        assert_eq!(word_positions("sta pa", "Git › Status page"), vec![6, 7, 8, 13, 14]);
    }

    /// Words match word starts first, then anywhere in the label, then a
    /// description's words; a scattered match across the label doesn't.
    #[test]
    fn word_matching_prefers_word_starts_and_ignores_scatter() {
        let git = word_match("git", "Git › Status page git.open", "Show the Git status").unwrap();
        let elsewhere = word_match("git", "Explorer › Make this the project and open Git", "").unwrap();
        assert!(git > elsewhere, "the area's own first");
        assert!(word_match("git", "Debug › Breakpoint hits debug.breakpoint_hits", "Give the line's breakpoint a hit count").is_none(), "letters scattered over the label aren't a match");
        assert!(word_match("hit", "Debug › Breakpoint hits", "").unwrap() > word_match("count", "Debug › Breakpoint hits", "Give it a hit count").unwrap());
        assert!(word_match("gst", "Git › Status page", "").is_some(), "letters running together still match");
        assert!(word_match("status git", "Git › Status page", "").is_some(), "words in any order");
    }

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
