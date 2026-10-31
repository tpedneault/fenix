//! Conflict markers in a working-tree file, and resolving one by
//! keeping a side.
//!
//! Pure text in, pure text out -- no `git` invocation and no file I/O,
//! so every shape a conflict can take (nested-looking content, the
//! `diff3` style's extra base section, an unterminated region, CRLF)
//! is testable directly rather than by staging a real merge for each.
//!
//! The markers are git's own, documented in `git merge`'s
//! "HOW CONFLICTS ARE PRESENTED":
//!
//! ```text
//! <<<<<<< HEAD
//! our side
//! ||||||| merged common ancestors     (diff3/zdiff3 style only)
//! what both sides started from
//! =======
//! their side
//! >>>>>>> other-branch
//! ```

/// One conflicted region, as line indices into the text it was found in.
///
/// Every index is a *line* number, zero-based, and every range is
/// half-open -- `ours` is `ours_start..ours_end`, and so on. Line
/// indices rather than byte offsets because that's what a cursor in the
/// editor is expressed in, and what "jump to the next conflict" needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The `<<<<<<<` line.
    pub start: usize,
    /// Our side's content, between `<<<<<<<` and the base/`=======`.
    pub ours: std::ops::Range<usize>,
    /// The common ancestor's content, present only in `diff3`/`zdiff3`
    /// merge styles (the `|||||||` section). `None` in the default
    /// style, which omits it.
    pub base: Option<std::ops::Range<usize>>,
    /// Their side's content, between `=======` and `>>>>>>>`.
    pub theirs: std::ops::Range<usize>,
    /// The `>>>>>>>` line.
    pub end: usize,
}

impl Conflict {
    /// Whether `line` falls anywhere inside this conflict, markers
    /// included -- what "the conflict under the cursor" means.
    pub fn contains(&self, line: usize) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

/// Which side to keep when resolving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Keep `HEAD`'s version -- what you had before the merge/rebase
    /// started. Note that during a *rebase* git labels the sides the
    /// other way round from what most people expect (`HEAD` is the
    /// branch being replayed onto), which is exactly why the UI names
    /// these "ours"/"theirs" after the markers rather than after
    /// branches.
    Ours,
    Theirs,
    /// Keep both, ours first -- for the common case where the two sides
    /// added different things in the same place and both belong.
    Both,
}

/// Every conflicted region in `text`, in the order they appear.
///
/// Markers are recognized at the start of a line, as git writes them.
/// A region with no closing `>>>>>>>` is skipped rather than guessed
/// at: an unterminated conflict means the file was hand-edited into a
/// state this can't reason about, and silently "resolving" it could
/// throw away work.
pub fn find_conflicts(text: &str) -> Vec<Conflict> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut conflicts = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].starts_with("<<<<<<<") {
            i += 1;
            continue;
        }
        let start = i;
        let mut base_marker = None;
        let mut separator = None;
        let mut end = None;
        for (j, line) in lines.iter().enumerate().skip(start + 1) {
            if line.starts_with("<<<<<<<") {
                // A second opener before this one closed: the first was
                // never terminated, so abandon it and restart here.
                break;
            } else if line.starts_with("|||||||") && separator.is_none() {
                base_marker = Some(j);
            } else if line.starts_with("=======") && separator.is_none() {
                separator = Some(j);
            } else if line.starts_with(">>>>>>>") {
                end = Some(j);
                break;
            }
        }
        match (separator, end) {
            (Some(sep), Some(end)) => {
                let ours_end = base_marker.unwrap_or(sep);
                conflicts.push(Conflict {
                    start,
                    ours: start + 1..ours_end,
                    base: base_marker.map(|b| b + 1..sep),
                    theirs: sep + 1..end,
                    end,
                });
                i = end + 1;
            }
            // Unterminated (or missing its `=======`): step past the
            // opener and keep looking, rather than swallowing the rest
            // of the file into a conflict that isn't one.
            _ => i = start + 1,
        }
    }
    conflicts
}

/// `text` with `conflict` replaced by the chosen side, markers removed.
///
/// The rest of the file is untouched, including its line endings: this
/// splices whole lines out of the original rather than rebuilding them,
/// so a CRLF file stays CRLF (the `\r` lives at the end of each line's
/// own text) and a file with no trailing newline keeps not having one.
pub fn resolve_conflict(text: &str, conflict: &Conflict, resolution: Resolution) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if conflict.end >= lines.len() {
        return text.to_string();
    }
    let kept: Vec<&str> = match resolution {
        Resolution::Ours => lines[conflict.ours.clone()].to_vec(),
        Resolution::Theirs => lines[conflict.theirs.clone()].to_vec(),
        Resolution::Both => {
            let mut both = lines[conflict.ours.clone()].to_vec();
            both.extend_from_slice(&lines[conflict.theirs.clone()]);
            both
        }
    };
    let mut out: Vec<&str> = lines[..conflict.start].to_vec();
    out.extend(kept);
    out.extend_from_slice(&lines[conflict.end + 1..]);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &str = "before\n<<<<<<< HEAD\nour line\n=======\ntheir line\n>>>>>>> feature\nafter\n";

    #[test]
    fn finds_a_single_conflict_and_its_two_sides() {
        let conflicts = find_conflicts(SIMPLE);
        assert_eq!(conflicts.len(), 1);
        let c = &conflicts[0];
        assert_eq!(c.start, 1);
        assert_eq!(c.ours, 2..3);
        assert_eq!(c.theirs, 4..5);
        assert_eq!(c.end, 5);
        assert_eq!(c.base, None, "the default merge style has no base section");
    }

    #[test]
    fn keeping_ours_removes_the_markers_and_their_side() {
        let out = resolve_conflict(SIMPLE, &find_conflicts(SIMPLE)[0], Resolution::Ours);
        assert_eq!(out, "before\nour line\nafter\n");
    }

    #[test]
    fn keeping_theirs_removes_the_markers_and_our_side() {
        let out = resolve_conflict(SIMPLE, &find_conflicts(SIMPLE)[0], Resolution::Theirs);
        assert_eq!(out, "before\ntheir line\nafter\n");
    }

    #[test]
    fn keeping_both_keeps_our_side_first() {
        let out = resolve_conflict(SIMPLE, &find_conflicts(SIMPLE)[0], Resolution::Both);
        assert_eq!(out, "before\nour line\ntheir line\nafter\n");
    }

    #[test]
    fn a_resolved_file_has_no_conflicts_left_in_it() {
        let out = resolve_conflict(SIMPLE, &find_conflicts(SIMPLE)[0], Resolution::Ours);
        assert!(find_conflicts(&out).is_empty());
    }

    #[test]
    fn finds_several_conflicts_and_resolving_one_leaves_the_others() {
        let text = "a\n<<<<<<< HEAD\nx1\n=======\ny1\n>>>>>>> b\nmiddle\n<<<<<<< HEAD\nx2\n=======\ny2\n>>>>>>> b\nz\n";
        let conflicts = find_conflicts(text);
        assert_eq!(conflicts.len(), 2);

        let out = resolve_conflict(text, &conflicts[0], Resolution::Theirs);
        assert_eq!(out, "a\ny1\nmiddle\n<<<<<<< HEAD\nx2\n=======\ny2\n>>>>>>> b\nz\n");
        assert_eq!(find_conflicts(&out).len(), 1, "the second is still there");
    }

    #[test]
    fn the_diff3_styles_base_section_is_recognized_and_never_kept() {
        let text = "<<<<<<< HEAD\nours\n||||||| merged common ancestors\nthe original\n=======\ntheirs\n>>>>>>> other\n";
        let conflicts = find_conflicts(text);
        let c = &conflicts[0];
        assert_eq!(c.ours, 1..2, "our side stops at the base marker, not the separator");
        assert_eq!(c.base, Some(3..4));
        assert_eq!(c.theirs, 5..6);

        // The ancestor is context, never content to keep.
        assert_eq!(resolve_conflict(text, c, Resolution::Ours), "ours\n");
        assert_eq!(resolve_conflict(text, c, Resolution::Both), "ours\ntheirs\n");
    }

    #[test]
    fn a_multi_line_side_is_kept_whole() {
        let text = "<<<<<<< HEAD\nour 1\nour 2\n=======\ntheir 1\ntheir 2\ntheir 3\n>>>>>>> b\n";
        let c = &find_conflicts(text)[0];
        assert_eq!(resolve_conflict(text, c, Resolution::Ours), "our 1\nour 2\n");
        assert_eq!(resolve_conflict(text, c, Resolution::Theirs), "their 1\ntheir 2\ntheir 3\n");
    }

    #[test]
    fn an_empty_side_resolves_to_nothing_rather_than_a_blank_line() {
        // One side deleted what the other changed -- keeping the empty
        // side means the region goes away entirely.
        let text = "before\n<<<<<<< HEAD\n=======\ntheirs\n>>>>>>> b\nafter\n";
        let c = &find_conflicts(text)[0];
        assert_eq!(resolve_conflict(text, c, Resolution::Ours), "before\nafter\n");
    }

    #[test]
    fn crlf_line_endings_survive_resolution() {
        // The `\r` is part of each line's own text, so splicing whole
        // lines preserves it -- a resolved CRLF file must not come back
        // half-converted.
        let text = "before\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> b\r\nafter\r\n";
        let c = &find_conflicts(text)[0];
        assert_eq!(resolve_conflict(text, c, Resolution::Theirs), "before\r\ntheirs\r\nafter\r\n");
    }

    #[test]
    fn a_file_with_no_trailing_newline_keeps_not_having_one() {
        let text = "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> b";
        let c = &find_conflicts(text)[0];
        assert_eq!(resolve_conflict(text, c, Resolution::Ours), "ours");
    }

    #[test]
    fn text_with_no_conflicts_yields_none() {
        assert!(find_conflicts("just\nsome\nlines\n").is_empty());
        assert!(find_conflicts("").is_empty());
    }

    #[test]
    fn an_unterminated_conflict_is_skipped_rather_than_guessed_at() {
        // Hand-edited into a state this can't reason about: resolving it
        // would mean deciding where the region ends, and getting that
        // wrong deletes real work.
        let text = "<<<<<<< HEAD\nours\n=======\ntheirs\nno closing marker\n";
        assert!(find_conflicts(text).is_empty());
    }

    #[test]
    fn a_second_opener_before_the_first_closed_starts_a_fresh_region() {
        let text = "<<<<<<< HEAD\nstray\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> b\n";
        let conflicts = find_conflicts(text);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].start, 2, "the well-formed region is the one found");
    }

    #[test]
    fn contains_covers_the_markers_as_well_as_the_content() {
        let c = &find_conflicts(SIMPLE)[0];
        assert!(c.contains(1), "the <<<<<<< line");
        assert!(c.contains(3), "the ======= line");
        assert!(c.contains(5), "the >>>>>>> line");
        assert!(!c.contains(0));
        assert!(!c.contains(6));
    }

    #[test]
    fn resolving_a_conflict_whose_lines_are_out_of_range_leaves_the_text_alone() {
        let bogus = Conflict { start: 0, ours: 1..2, base: None, theirs: 3..4, end: 99 };
        assert_eq!(resolve_conflict(SIMPLE, &bogus, Resolution::Ours), SIMPLE);
    }
}
