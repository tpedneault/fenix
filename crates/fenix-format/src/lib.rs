//! Structural, language-independent reindentation -- Emacs' `indent-
//! region` (equivalently, real Vim's `=` operator), not a per-language
//! external formatter. This crate used to shell out to `tclfmt` for
//! Tcl and nothing else; that's gone, replaced outright rather than
//! kept as a fallback, since a formatter that only ever covered one
//! language (and needed a separate binary installed to work at all)
//! wasn't a tool worth keeping around once a generic one exists.
//!
//! The algorithm: walk the source top to bottom tracking a running
//! `{`/`(`/`[` nesting depth, and set each touched line's leading
//! whitespace to `depth * indent_width` spaces (a line starting with
//! closing brackets dedents itself by however many precede its first
//! real character, same as any bracket-aware editor's reindent). No
//! per-language grammar or keyword rules -- it works identically for
//! Tcl's `{}`, C's `{}`/`()`, JSON's `{}`/`[]`, or anything else built
//! on nested delimiters, which is what makes it usable on a buffer of
//! any (or no) detected language rather than needing one formatter per
//! language wired in.

use std::ops::Range;

fn is_opening_bracket(c: char) -> bool {
    matches!(c, '{' | '(' | '[')
}

fn is_closing_bracket(c: char) -> bool {
    matches!(c, '}' | ')' | ']')
}

/// How many leading closing-bracket characters `trimmed` (already
/// stripped of its own leading whitespace) starts with -- `})` at a
/// line's start dedents that line by 2 levels before its own content
/// is considered, not just 1, matching how multiple nested calls
/// closing on one line actually read.
fn leading_closer_count(trimmed: &str) -> usize {
    trimmed.chars().take_while(|&c| is_closing_bracket(c)).count()
}

/// Recomputes the leading whitespace of every line in `first_line..=
/// last_line` (0-indexed, inclusive) from bracket-nesting depth --
/// `SPC c f`'s Visual-selection case passes the selection's own line
/// span; `SPC c F` passes the whole buffer's.
///
/// Depth is tracked across the *entire* `source`, not just the touched
/// range: nesting is a property of the whole file, so a selection that
/// starts three levels deep needs to know that, not treat itself as
/// starting fresh at depth 0. Lines outside the touched range are left
/// completely untouched (not even trailing-whitespace-trimmed) but
/// still count toward the running depth for the lines that follow.
///
/// `skip` is a list of byte ranges (typically every string/comment span
/// from the caller's own syntax highlighter, e.g. `SyntaxState::
/// highlights_in_range`) to treat as opaque -- a `{` sitting inside a
/// string literal or a comment doesn't perturb the depth count. An
/// empty `skip` still works, just less accurately (every bracket-shaped
/// character counts, literal or not) -- the caller passes one whenever
/// it has real syntax info and an empty slice otherwise, never a hard
/// requirement.
///
/// Blank (or whitespace-only) lines are left exactly as they are --
/// no leading whitespace manufactured onto an otherwise-empty line --
/// and don't affect depth either way, matching how `gcc`'s own comment
/// toggling and every other line-batch operation in this codebase
/// already treats them.
///
/// Never fails: there's no external tool to be missing, and any
/// otherwise-degenerate input (`first_line > last_line`, either out of
/// the file's actual range, unbalanced brackets) just does the most
/// sensible possible thing rather than erroring -- see the tests for
/// exactly what that is in each case.
pub fn reindent(source: &str, first_line: usize, last_line: usize, indent_width: usize, skip: &[Range<usize>]) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len());
    let mut depth: usize = 0;
    let mut byte_pos: usize = 0;

    for (line_no, line) in lines.iter().enumerate() {
        let line_start = byte_pos;
        let content_start = line.len() - line.trim_start().len();
        let trimmed = &line[content_start..];

        if trimmed.is_empty() {
            out_lines.push((*line).to_string());
        } else if line_no >= first_line && line_no <= last_line {
            let line_depth = depth.saturating_sub(leading_closer_count(trimmed));
            let mut rewritten = " ".repeat(line_depth * indent_width);
            rewritten.push_str(trimmed);
            out_lines.push(rewritten);
        } else {
            out_lines.push((*line).to_string());
        }

        for (offset, ch) in line.char_indices() {
            if skip.iter().any(|r| r.contains(&(line_start + offset))) {
                continue;
            }
            if is_opening_bracket(ch) {
                depth += 1;
            } else if is_closing_bracket(ch) {
                depth = depth.saturating_sub(1);
            }
        }

        // +1 for the '\n' this line was split on -- meaningless (and
        // never read) past the last line, since nothing byte-indexes
        // beyond the end of `source`.
        byte_pos += line.len() + 1;
    }

    out_lines.join("\n")
}

/// Same algorithm as `reindent`, but returns only the resulting text
/// of lines `first_line..=last_line` (joined with `'\n'`, no leading or
/// trailing newline of its own) instead of the whole document.
///
/// What a caller reindenting a Visual selection actually wants: the
/// surrounding file still has to be walked for correct nesting depth
/// (`reindent`'s own doc comment), but only the selection's own lines
/// should become part of the edit that lands in the buffer -- splicing
/// this back in via a precise range-replace keeps the undo step (and
/// the cursor/mark bookkeeping that rides along with it) scoped to
/// what was actually selected, the same as any other Visual operator.
///
/// `first_line > last_line`, or either past the end of `source`,
/// degrades to an empty string -- there's nothing to splice in, not a
/// panic.
pub fn reindent_lines(source: &str, first_line: usize, last_line: usize, indent_width: usize, skip: &[Range<usize>]) -> String {
    let full = reindent(source, first_line, last_line, indent_width, skip);
    let count = (last_line + 1).saturating_sub(first_line);
    full.split('\n').skip(first_line).take(count).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reindents_simple_nested_blocks_from_scratch() {
        let source = "proc foo {a b} {\nif {$a} {\nputs hi\n}\n}";
        let got = reindent(source, 0, 4, 4, &[]);
        assert_eq!(got, "proc foo {a b} {\n    if {$a} {\n        puts hi\n    }\n}");
    }

    #[test]
    fn a_line_starting_with_a_closing_bracket_dedents_itself_before_its_own_content() {
        // Deliberately flush-left to start -- proves the dedent-on-
        // leading-closer rule actively produces the indent, not just
        // that it leaves already-correct input alone.
        let source = "foo(\nbar,\n)";
        let got = reindent(source, 0, 2, 4, &[]);
        // Depth is 1 by the time line 2 ("`)`") is reached -- its own
        // one leading closer brings that line's own indent back to 0,
        // matching every bracket-aware editor's convention, without
        // affecting line 1's indent (still inside the open paren).
        assert_eq!(got, "foo(\n    bar,\n)");
    }

    #[test]
    fn multiple_leading_closers_dedent_by_that_many_levels_at_once() {
        let source = "a({\nb\n})";
        let got = reindent(source, 0, 2, 4, &[]);
        // Two opening brackets on line 0 -- line 1 sits two levels
        // deep (8 spaces), and the two leading closers on line 2 drop
        // straight back to 0, not just one level.
        assert_eq!(got, "a({\n        b\n})");
    }

    #[test]
    fn blank_lines_are_left_alone_and_do_not_gain_manufactured_whitespace() {
        let source = "a {\n\nb\n}";
        let got = reindent(source, 0, 3, 4, &[]);
        assert_eq!(got, "a {\n\n    b\n}");
    }

    #[test]
    fn only_lines_inside_the_requested_range_are_rewritten() {
        // Line 0 is badly misindented on purpose -- outside the
        // requested 1..=2 range, it must come back completely
        // untouched, not "fixed" as a side effect.
        let source = "      a {\nb\nc\n}";
        let got = reindent(source, 1, 2, 4, &[]);
        assert_eq!(got, "      a {\n    b\n    c\n}");
    }

    #[test]
    fn depth_from_outside_the_touched_range_still_counts() {
        // Line 0 (untouched) opens a level; only line 1 is in range,
        // and its indent must reflect having started one level deep,
        // not depth 0 just because line 0 wasn't itself rewritten.
        let source = "a {\nb\n}";
        let got = reindent(source, 1, 1, 4, &[]);
        assert_eq!(got, "a {\n    b\n}");
    }

    #[test]
    fn fixes_indentation_that_has_drifted_out_of_alignment() {
        // The exact complaint this exists to fix: hand-typed or
        // repeatedly `>>`/`<<`-shifted lines that no longer agree with
        // each other or with a clean multiple of the indent width.
        let source = "if {1} {\n  puts a\n     puts b\nputs c\n}";
        let got = reindent(source, 0, 4, 4, &[]);
        assert_eq!(got, "if {1} {\n    puts a\n    puts b\n    puts c\n}");
    }

    #[test]
    fn a_custom_indent_width_is_honored() {
        let source = "a {\nb\n}";
        let got = reindent(source, 0, 2, 2, &[]);
        assert_eq!(got, "a {\n  b\n}");
    }

    #[test]
    fn reindenting_already_correct_indentation_is_a_no_op() {
        let source = "a {\n    b {\n        c\n    }\n}";
        let got = reindent(source, 0, 4, 4, &[]);
        assert_eq!(got, source);
    }

    #[test]
    fn a_bracket_inside_a_skip_range_does_not_affect_depth() {
        // `"{ not real"` on line 0 -- without `skip`, that `{` would
        // bump depth and wrongly indent line 1. The byte range covers
        // exactly the quoted string, the same shape a syntax
        // highlighter's own string-span capture would report.
        let source = "puts \"{ not real\"\nb";
        let unskipped = reindent(source, 0, 1, 4, &[]);
        assert_eq!(unskipped, "puts \"{ not real\"\n    b", "sanity check: without skip, the stray brace does bump depth");

        let skip = [5..17]; // byte range of the quoted string, including both quotes
        let skipped = reindent(source, 0, 1, 4, &skip);
        assert_eq!(skipped, source, "with the string excluded, there's nothing to indent from");
    }

    #[test]
    fn unbalanced_closing_brackets_saturate_at_zero_rather_than_underflowing() {
        let source = "}}}\na";
        let got = reindent(source, 0, 1, 4, &[]);
        assert_eq!(got, "}}}\na");
    }

    #[test]
    fn an_empty_source_reindents_to_empty() {
        assert_eq!(reindent("", 0, 0, 4, &[]), "");
    }

    #[test]
    fn a_range_past_the_end_of_the_file_touches_nothing() {
        let source = "a\nb";
        assert_eq!(reindent(source, 5, 9, 4, &[]), source);
    }

    #[test]
    fn an_inverted_range_touches_nothing() {
        let source = "a {\nb\n}";
        assert_eq!(reindent(source, 2, 0, 4, &[]), source);
    }

    #[test]
    fn a_trailing_newline_is_preserved() {
        let source = "a {\n    b\n}\n";
        let got = reindent(source, 0, 3, 4, &[]);
        assert_eq!(got, source);
    }

    #[test]
    fn reindent_lines_returns_only_the_requested_lines_own_text() {
        let source = "if {1} {\nputs a\nputs b\n}";
        let got = reindent_lines(source, 1, 2, 4, &[]);
        assert_eq!(got, "    puts a\n    puts b");
    }

    #[test]
    fn reindent_lines_still_uses_the_whole_file_for_nesting_context() {
        // Line 0 (not part of the requested range) opens a level --
        // the extracted line 1 must still come back indented one
        // level, not computed as if it started the file at depth 0.
        let source = "a {\nb\n}";
        let got = reindent_lines(source, 1, 1, 4, &[]);
        assert_eq!(got, "    b");
    }

    #[test]
    fn reindent_lines_of_a_single_line_selection_returns_just_that_line() {
        let source = "a {\nb\n}";
        assert_eq!(reindent_lines(source, 0, 0, 4, &[]), "a {");
    }

    #[test]
    fn reindent_lines_with_an_inverted_range_is_empty() {
        let source = "a {\nb\n}";
        assert_eq!(reindent_lines(source, 2, 0, 4, &[]), "");
    }
}
