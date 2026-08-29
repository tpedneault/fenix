use fenix_core::{Buffer, Cursor};
use regex::Regex;

use crate::motion;
use crate::state::VimEvent;

/// Parses and runs one confirmed `:` command line -- the bare `w`/`q`/
/// `q!`/`wq`/`x` buffer-close commands and their whole-app `qa`/
/// `quitall`/`qa!`/`quitall!`/`wqa`/`xa` counterparts (see `VimEvent`'s
/// own doc comments for the buffer-vs-app distinction), `set shiftwidth=N`/`set sw=N`
/// (mutates `indent_width` in place, since this is a plain function
/// without access to `VimState`'s own field), or a `[range]s/pattern/
/// replacement/flags` substitute (`last_search`, the most recently
/// confirmed `/`/`?` pattern if any, is what an empty `s///` pattern
/// falls back to -- see `run_substitute`). Anything else is silently
/// ignored, matching this project's existing "log and degrade, never
/// crash on user input" posture elsewhere (e.g. `fenix-explorer`'s
/// file-op failures).
pub fn run_ex_command(
    cmd: &str,
    buffer: &mut Buffer,
    cursor: &mut Cursor,
    indent_width: &mut usize,
    last_search: Option<&str>,
) -> VimEvent {
    let cmd = cmd.trim();
    match cmd {
        "w" => return VimEvent::RequestSave,
        "q" => return VimEvent::RequestCloseBuffer,
        "q!" => return VimEvent::RequestForceCloseBuffer,
        "wq" | "x" => return VimEvent::RequestSaveAndCloseBuffer,
        "qa" | "quitall" => return VimEvent::RequestQuitAll,
        "qa!" | "quitall!" => return VimEvent::RequestForceQuitAll,
        "wqa" | "xa" => return VimEvent::RequestSaveAllAndQuit,
        _ => {}
    }

    if cmd == "set" || cmd.starts_with("set ") {
        let rest = cmd["set".len()..].trim_start();
        if apply_set_command(rest, indent_width) {
            return VimEvent::IndentWidthChanged(*indent_width);
        }
        return VimEvent::None;
    }

    // `:g!`/`:v` before bare `:g` -- otherwise the `!`/`v` would be
    // swallowed as part of the pattern delimiter by the plain `g`
    // branch. Unlike `:s`, `:g` defaults to the *whole buffer* with no
    // range prefix (matching real Vim); range-restricted `:N,Mg/.../`
    // isn't supported here, same scope-limited posture `parse_range`'s
    // own doc comment already states for `:s`.
    if let Some(rest) = cmd.strip_prefix("g!").or_else(|| cmd.strip_prefix('v')) {
        return run_global(buffer, cursor, rest, true, last_search);
    }
    if let Some(rest) = cmd.strip_prefix('g') {
        return run_global(buffer, cursor, rest, false, last_search);
    }

    let (range, rest) = parse_range(cmd, motion::last_line(buffer));
    if let Some(spec) = rest.strip_prefix('s') {
        let (cur_line, _) = buffer.line_col(cursor);
        let (start_line, end_line) = range.unwrap_or((cur_line, cur_line));
        return run_substitute(buffer, cursor, start_line, end_line, spec, last_search);
    }
    VimEvent::None
}

/// `:g/pattern/subcommand` (`invert` -- `:g!`/`:v` -- keeps the
/// *non*-matching lines instead). `rest` is everything after the `g`/
/// `g!`/`v`, delimiter-first exactly like `run_substitute`'s own `spec`
/// (an empty pattern falls back to `last_search`). Scope: exactly two
/// subcommands, matching this file's own established "common cases,
/// not the full Ex language" posture --
///
/// - `d`: deletes every matching line.
/// - `s/old/new/flags`: runs the *existing* `run_substitute` once per
///   matching line -- substitution never changes line count, so the
///   original match list stays valid across the whole loop.
///
/// Anything else -- including a bare `:g/pattern/` with no subcommand,
/// real Vim's own "print matching lines" default (meaningless here,
/// there's no line-listing UI to print into) -- is a silent no-op, not
/// a default to `d`: an unrecognized subcommand silently deleting the
/// user's lines would be a dangerous surprise.
fn run_global(buffer: &mut Buffer, cursor: &mut Cursor, rest: &str, invert: bool, last_search: Option<&str>) -> VimEvent {
    let Some(delim) = rest.chars().next() else { return VimEvent::None };
    let parts: Vec<&str> = rest[delim.len_utf8()..].splitn(2, delim).collect();
    let pattern = parts.first().copied().unwrap_or("");
    let pattern = if pattern.is_empty() { last_search.unwrap_or("") } else { pattern };
    if pattern.is_empty() {
        return VimEvent::None;
    }
    let subcommand = parts.get(1).copied().unwrap_or("").trim();

    let re = match Regex::new(pattern) {
        Ok(re) => re,
        Err(err) => return VimEvent::Error(format!(":g pattern error: {err}")),
    };
    let matching: Vec<usize> = (0..buffer.line_count())
        .filter(|&line| {
            let start = buffer.line_start_char(line);
            let text = buffer.text_range(start, start + buffer.line_len(line));
            re.is_match(&text) != invert
        })
        .collect();
    if matching.is_empty() {
        return VimEvent::None;
    }

    if subcommand == "d" {
        // Highest index first: each deletion only ever shifts indices
        // already processed, never the ones still queued, so `line_
        // start_char`/`line_count` stay correct for every remaining
        // (lower) match with no reindexing needed -- both are re-
        // queried fresh here, deliberately, rather than cached from
        // before the loop started.
        for &line in matching.iter().rev() {
            let start = buffer.line_start_char(line);
            let end = if line + 1 < buffer.line_count() { buffer.line_start_char(line + 1) } else { buffer.len_chars() };
            buffer.delete_range(cursor, start, end);
        }
        cursor.char_idx = cursor.char_idx.min(buffer.len_chars().saturating_sub(1));
        return VimEvent::None;
    }
    if let Some(spec) = subcommand.strip_prefix('s') {
        let mut event = VimEvent::None;
        for &line in &matching {
            let result = run_substitute(buffer, cursor, line, line, spec, last_search);
            if matches!(result, VimEvent::Error(_)) {
                event = result;
            }
        }
        return event;
    }
    VimEvent::None
}

/// `:set shiftwidth=N` / `:set sw=N` -- the one `:set` option this
/// editor has (see `indent.rs`'s own doc comment for why `expandtab`/
/// `noexpandtab` aren't offered: no tab-stop-aware rendering to make a
/// literal tab character look right). Mutates `width` in place; returns
/// whether it actually changed. An unrecognized token, or a value that
/// doesn't parse as a positive integer, is silently ignored -- same
/// "ignore rather than error" posture every other `:` command has here,
/// there being no status-line/error-message UI to report through yet.
fn apply_set_command(rest: &str, width: &mut usize) -> bool {
    let mut changed = false;
    for token in rest.split_whitespace() {
        if let Some(value) = token.strip_prefix("shiftwidth=").or_else(|| token.strip_prefix("sw=")) {
            if let Ok(new_width) = value.parse::<usize>() {
                if new_width > 0 && new_width != *width {
                    *width = new_width;
                    changed = true;
                }
            }
        }
    }
    changed
}

/// Splits a leading Ex range off `cmd`: `%` (whole buffer, `(0,
/// last_line)`) or a plain `N,M` two-number range (1-indexed in the
/// command text, converted to 0-indexed line numbers here) -- Vim
/// supports much more (marks, `'<,'>`, relative offsets like `.+1`);
/// this covers the common cases without needing any of that (see the
/// plan's own Scope/Out for why). `None` (no range recognized) leaves
/// `cmd` untouched for the caller to default to "current line only".
fn parse_range(cmd: &str, last_line: usize) -> (Option<(usize, usize)>, &str) {
    if let Some(rest) = cmd.strip_prefix('%') {
        return (Some((0, last_line)), rest);
    }
    let digits_end = |s: &str| s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let end1 = digits_end(cmd);
    if end1 == 0 {
        return (None, cmd);
    }
    let Ok(n1) = cmd[..end1].parse::<usize>() else { return (None, cmd) };
    let after1 = &cmd[end1..];
    let Some(after_comma) = after1.strip_prefix(',') else { return (None, cmd) };
    let end2 = digits_end(after_comma);
    if end2 == 0 {
        return (None, cmd);
    }
    let Ok(n2) = after_comma[..end2].parse::<usize>() else { return (None, cmd) };
    (Some((n1.saturating_sub(1).min(last_line), n2.saturating_sub(1).min(last_line))), &after_comma[end2..])
}

/// `spec` is everything after the `s` in `s/pat/repl/flags` -- the first
/// character is the delimiter (Vim convention: any punctuation works,
/// not hardcoded to `/`, so a pattern that itself contains `/` can use
/// e.g. `s#/path#/other#`). An empty pattern (`s//repl/`) falls back to
/// `last_search`, matching real Vim; still a no-op (not an error -- there
/// was nothing to reuse) if that's also `None`.
fn run_substitute(
    buffer: &mut Buffer,
    cursor: &mut Cursor,
    start_line: usize,
    end_line: usize,
    spec: &str,
    last_search: Option<&str>,
) -> VimEvent {
    let Some(delim) = spec.chars().next() else { return VimEvent::None };
    let parts: Vec<&str> = spec[delim.len_utf8()..].splitn(3, delim).collect();
    let pattern = parts.first().copied().unwrap_or("");
    let pattern = if pattern.is_empty() { last_search.unwrap_or("") } else { pattern };
    if pattern.is_empty() {
        return VimEvent::None;
    }
    let replacement = parts.get(1).copied().unwrap_or("");
    let flags = parts.get(2).copied().unwrap_or("");

    let spec = SubstituteSpec {
        start_line,
        end_line,
        pattern,
        replacement,
        global: flags.contains('g'),
        ignore_case: flags.contains('i'),
    };
    match substitute(buffer, cursor, &spec) {
        Ok(()) => VimEvent::None,
        Err(err) => VimEvent::Error(format!(":s failed: {err}")),
    }
}

/// Bundles `:s`'s parsed pieces -- `run_substitute`'s own five separate
/// values would trip clippy's too-many-arguments lint on `substitute`
/// once `buffer`/`cursor` are added, and a named struct reads better at
/// the call site than a five-element tuple would anyway.
struct SubstituteSpec<'a> {
    start_line: usize,
    end_line: usize,
    pattern: &'a str,
    replacement: &'a str,
    global: bool,
    ignore_case: bool,
}

/// The actual substitution: extracts the target line range as one
/// `String`, runs `replace_in_text` entirely in memory, and -- only if
/// anything actually changed -- swaps the whole span back into the rope
/// with one `Buffer::replace_range` call (one atomic undo step,
/// regardless of how many lines/matches were touched). Doing the regex
/// work against a plain string and only touching the real rope once
/// sidesteps every incremental-offset-drift issue that editing line by
/// line in place would create.
fn substitute(buffer: &mut Buffer, cursor: &mut Cursor, spec: &SubstituteSpec) -> Result<(), regex::Error> {
    let end_line = spec.end_line.min(motion::last_line(buffer));
    let (start_line, end_line) =
        if spec.start_line <= end_line { (spec.start_line, end_line) } else { (end_line, spec.start_line) };

    let start_char = buffer.line_start_char(start_line);
    let end_char = buffer.line_start_char(end_line) + buffer.line_len(end_line);
    let original = buffer.text_range(start_char, end_char);

    let (new_text, count) = replace_in_text(&original, spec.pattern, spec.replacement, spec.ignore_case, spec.global)?;

    if count > 0 {
        buffer.replace_range(cursor, start_char, end_char, &new_text);
        // Real Vim leaves the cursor on the last substituted line's
        // first non-blank -- `replace_range` already left it somewhere
        // in the new text (end of the inserted span), so re-derive the
        // line from that and land properly.
        let target_line = buffer.line_col(cursor).0;
        cursor.char_idx = motion::line_first_non_blank(buffer, target_line);
        let (_, col) = buffer.line_col(cursor);
        cursor.sticky_col = col;
    }
    Ok(())
}

/// Pure text-in/text-out find+replace -- the engine both `:s`'s own
/// `substitute()` above and any host-level search/replace UI
/// (`fenix-gui`'s `SPC s r`/`SPC s p`) share, so there's exactly one
/// implementation of "how Fenix does find+replace," not a second,
/// possibly-divergent one. `global` mirrors the `g` flag (every match
/// per line, not just the first), `ignore_case` the `i` flag -- same
/// semantics as real Vim's `:s`, including that a match is always
/// scoped to a single line (`text` is split on `\n` and each line
/// processed independently, exactly like `substitute()` always has) --
/// a pattern can't match across a newline. Returns the transformed text
/// and how many replacements were made (`0` and `text` unchanged if the
/// pattern matched nothing).
pub fn replace_in_text(
    text: &str,
    pattern: &str,
    replacement: &str,
    ignore_case: bool,
    global: bool,
) -> Result<(String, usize), regex::Error> {
    let pattern_src = if ignore_case { format!("(?i){pattern}") } else { pattern.to_string() };
    let re = Regex::new(&pattern_src)?;
    let translated = translate_replacement(replacement);

    let mut count = 0usize;
    let new_text: String = text
        .split('\n')
        .map(|line| {
            if global {
                count += re.find_iter(line).count();
                re.replace_all(line, translated.as_str()).into_owned()
            } else {
                if re.is_match(line) {
                    count += 1;
                }
                re.replace(line, translated.as_str()).into_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok((new_text, count))
}

/// Translates Vim's own `:s` replacement syntax into the `regex` crate's
/// `$`-based one, so what Thomas types matches real Vim muscle memory:
/// `&` (whole match) -> `${0}`, `\1`-`\9` (capture groups) -> `${1}`-
/// `${9}`, `\&`/`\\` -> literal `&`/`\`, and any literal `$` in the
/// input escaped to `$$` so the regex crate doesn't misinterpret it (it
/// has no meaning as a literal char in Vim's own replacement syntax, so
/// this is a pure implementation-detail escape, not a user-facing
/// feature).
pub fn translate_replacement(repl: &str) -> String {
    let mut out = String::new();
    let mut chars = repl.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(d) if d.is_ascii_digit() => {
                    out.push('$');
                    out.push('{');
                    out.push(d);
                    out.push('}');
                }
                Some('&') => out.push('&'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            },
            '&' => out.push_str("${0}"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::buf;

    fn cmd(text: &str) -> (Buffer, Cursor) {
        (buf(text), Cursor::at_start())
    }

    #[test]
    fn w_q_wq_x_close_the_buffer_not_the_app() {
        let (mut b, mut c) = cmd("hi");
        assert_eq!(run_ex_command("w", &mut b, &mut c, &mut 4, None), VimEvent::RequestSave);
        assert_eq!(run_ex_command("q", &mut b, &mut c, &mut 4, None), VimEvent::RequestCloseBuffer);
        assert_eq!(run_ex_command("q!", &mut b, &mut c, &mut 4, None), VimEvent::RequestForceCloseBuffer);
        assert_eq!(run_ex_command("wq", &mut b, &mut c, &mut 4, None), VimEvent::RequestSaveAndCloseBuffer);
        assert_eq!(run_ex_command("x", &mut b, &mut c, &mut 4, None), VimEvent::RequestSaveAndCloseBuffer);
    }

    #[test]
    fn qa_wqa_and_variants_target_the_whole_app() {
        let (mut b, mut c) = cmd("hi");
        assert_eq!(run_ex_command("qa", &mut b, &mut c, &mut 4, None), VimEvent::RequestQuitAll);
        assert_eq!(run_ex_command("quitall", &mut b, &mut c, &mut 4, None), VimEvent::RequestQuitAll);
        assert_eq!(run_ex_command("qa!", &mut b, &mut c, &mut 4, None), VimEvent::RequestForceQuitAll);
        assert_eq!(run_ex_command("quitall!", &mut b, &mut c, &mut 4, None), VimEvent::RequestForceQuitAll);
        assert_eq!(run_ex_command("wqa", &mut b, &mut c, &mut 4, None), VimEvent::RequestSaveAllAndQuit);
        assert_eq!(run_ex_command("xa", &mut b, &mut c, &mut 4, None), VimEvent::RequestSaveAllAndQuit);
    }

    #[test]
    fn substitute_with_no_range_affects_only_the_current_line() {
        let (mut b, mut c) = cmd("foo\nfoo\nfoo");
        c.char_idx = b.line_start_char(1); // on the second line
        run_ex_command("s/foo/bar/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "foo\nbar\nfoo");
    }

    #[test]
    fn substitute_with_percent_range_affects_the_whole_buffer() {
        let (mut b, mut c) = cmd("foo\nfoo\nfoo");
        run_ex_command("%s/foo/bar/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "bar\nbar\nbar");
    }

    #[test]
    fn substitute_with_a_numeric_range_affects_only_those_lines() {
        let (mut b, mut c) = cmd("foo\nfoo\nfoo\nfoo");
        run_ex_command("2,3s/foo/bar/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "foo\nbar\nbar\nfoo");
    }

    #[test]
    fn substitute_without_g_replaces_only_the_first_match_per_line() {
        let (mut b, mut c) = cmd("foo foo foo");
        run_ex_command("s/foo/bar/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "bar foo foo");
    }

    #[test]
    fn substitute_with_g_replaces_every_match_per_line() {
        let (mut b, mut c) = cmd("foo foo foo");
        run_ex_command("s/foo/bar/g", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "bar bar bar");
    }

    #[test]
    fn substitute_with_i_is_case_insensitive() {
        let (mut b, mut c) = cmd("FOO foo");
        run_ex_command("s/foo/bar/gi", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "bar bar");
    }

    #[test]
    fn substitute_supports_a_non_slash_delimiter() {
        let (mut b, mut c) = cmd("/usr/bin");
        run_ex_command("s#/usr#/opt#", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "/opt/bin");
    }

    #[test]
    fn substitute_supports_ampersand_and_backreferences() {
        // Pattern side uses the regex crate's own syntax (Scope's own
        // disclosed cut -- Vim's `\(...\)` capture-group escaping isn't
        // translated, only the *replacement* side is); `(...)` is
        // already a capture group here, no backslash needed.
        let (mut b, mut c) = cmd("hello world");
        run_ex_command(r"s/(\w+) (\w+)/\2 \1: [&]/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "world hello: [hello world]");
    }

    #[test]
    fn substitute_is_a_single_undo_step() {
        let (mut b, mut c) = cmd("foo foo foo");
        run_ex_command("s/foo/bar/g", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "bar bar bar");
        assert!(b.undo(&mut c));
        assert_eq!(b.text(), "foo foo foo");
    }

    #[test]
    fn substitute_with_no_matches_is_a_no_op() {
        let (mut b, mut c) = cmd("hello world");
        run_ex_command("s/xyz/abc/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn substitute_with_an_empty_pattern_is_a_no_op_when_theres_no_last_search() {
        let (mut b, mut c) = cmd("hello world");
        run_ex_command("s///", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn substitute_with_an_empty_pattern_reuses_the_last_search_pattern() {
        let (mut b, mut c) = cmd("hello world");
        run_ex_command("s//bye/", &mut b, &mut c, &mut 4, Some("hello"));
        assert_eq!(b.text(), "bye world");
    }

    // -- `:g`/`:g!`/`:v` --------------------------------------------------

    #[test]
    fn g_d_deletes_every_matching_line() {
        let (mut b, mut c) = cmd("keep\nDROP me\nkeep\nDROP me too\n");
        run_ex_command("g/DROP/d", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "keep\nkeep\n");
    }

    #[test]
    fn g_bang_d_deletes_the_non_matching_lines() {
        let (mut b, mut c) = cmd("keep\nDROP me\nkeep\nDROP me too\n");
        run_ex_command("g!/DROP/d", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "DROP me\nDROP me too\n");
    }

    #[test]
    fn v_is_an_alias_for_g_bang() {
        let (mut b, mut c) = cmd("keep\nDROP me\nkeep\nDROP me too\n");
        run_ex_command("v/DROP/d", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "DROP me\nDROP me too\n");
    }

    #[test]
    fn g_s_substitutes_on_every_matching_line_only() {
        let (mut b, mut c) = cmd("foo one\nbar two\nfoo three\n");
        run_ex_command("g/foo/s/foo/baz/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "baz one\nbar two\nbaz three\n");
    }

    #[test]
    fn g_with_an_unrecognized_subcommand_is_a_noop_not_a_delete() {
        // A missing/unknown subcommand must never default to deleting --
        // that would be a dangerous surprise for a typo'd command.
        let (mut b, mut c) = cmd("keep\nDROP me\n");
        run_ex_command("g/DROP/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "keep\nDROP me\n");
        run_ex_command("g/DROP/xyz", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "keep\nDROP me\n");
    }

    #[test]
    fn g_with_no_matches_is_a_noop() {
        let (mut b, mut c) = cmd("keep\nkeep too\n");
        run_ex_command("g/DROP/d", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "keep\nkeep too\n");
    }

    #[test]
    fn g_with_an_empty_pattern_reuses_the_last_search_pattern() {
        let (mut b, mut c) = cmd("keep\nDROP me\n");
        run_ex_command("g//d", &mut b, &mut c, &mut 4, Some("DROP"));
        assert_eq!(b.text(), "keep\n");
    }

    #[test]
    fn g_with_an_invalid_pattern_raises_an_error_event() {
        let (mut b, mut c) = cmd("a\nb\n");
        let event = run_ex_command("g/(unclosed/d", &mut b, &mut c, &mut 4, None);
        assert!(matches!(event, VimEvent::Error(_)), "expected an Error event, got {event:?}");
    }

    #[test]
    fn substitute_with_an_invalid_pattern_does_not_panic_or_change_the_buffer() {
        let (mut b, mut c) = cmd("hello world");
        run_ex_command("s/(unclosed/x/", &mut b, &mut c, &mut 4, None);
        assert_eq!(b.text(), "hello world");
    }

    #[test]
    fn substitute_with_an_invalid_pattern_raises_an_error_event() {
        let (mut b, mut c) = cmd("hello world");
        let event = run_ex_command("s/(unclosed/x/", &mut b, &mut c, &mut 4, None);
        assert!(matches!(event, VimEvent::Error(_)), "expected an Error event, got {event:?}");
    }

    #[test]
    fn replace_in_text_global_replaces_every_match_and_counts_them() {
        let (new_text, count) = replace_in_text("foo foo foo", "foo", "bar", false, true).unwrap();
        assert_eq!(new_text, "bar bar bar");
        assert_eq!(count, 3);
    }

    #[test]
    fn replace_in_text_non_global_replaces_only_the_first_match_per_line() {
        let (new_text, count) = replace_in_text("foo foo\nfoo foo", "foo", "bar", false, false).unwrap();
        assert_eq!(new_text, "bar foo\nbar foo");
        assert_eq!(count, 2); // one per line, not per match
    }

    #[test]
    fn replace_in_text_is_case_insensitive_when_asked() {
        let (new_text, count) = replace_in_text("FOO foo Foo", "foo", "bar", true, true).unwrap();
        assert_eq!(new_text, "bar bar bar");
        assert_eq!(count, 3);
    }

    #[test]
    fn replace_in_text_supports_ampersand_and_backreferences() {
        let (new_text, count) = replace_in_text("hello world", r"(\w+) (\w+)", r"\2 \1: [&]", false, false).unwrap();
        assert_eq!(new_text, "world hello: [hello world]");
        assert_eq!(count, 1);
    }

    #[test]
    fn replace_in_text_with_no_matches_returns_the_original_text_and_zero() {
        let (new_text, count) = replace_in_text("hello world", "xyz", "abc", false, true).unwrap();
        assert_eq!(new_text, "hello world");
        assert_eq!(count, 0);
    }

    #[test]
    fn replace_in_text_with_an_invalid_pattern_is_an_error_not_a_panic() {
        assert!(replace_in_text("hello", "(unclosed", "x", false, true).is_err());
    }

    #[test]
    fn translate_replacement_handles_ampersand_backreferences_and_escapes() {
        assert_eq!(translate_replacement("&"), "${0}");
        assert_eq!(translate_replacement(r"\1-\2"), "${1}-${2}");
        assert_eq!(translate_replacement(r"\&"), "&");
        assert_eq!(translate_replacement(r"\\"), "\\");
        assert_eq!(translate_replacement("$5"), "$$5");
        assert_eq!(translate_replacement("plain"), "plain");
    }

    #[test]
    fn set_shiftwidth_changes_the_indent_width_and_reports_it() {
        let (mut b, mut c) = cmd("hi");
        let mut width = 4;
        assert_eq!(run_ex_command("set shiftwidth=3", &mut b, &mut c, &mut width, None), VimEvent::IndentWidthChanged(3));
        assert_eq!(width, 3);
    }

    #[test]
    fn set_sw_is_an_accepted_alias_for_shiftwidth() {
        let (mut b, mut c) = cmd("hi");
        let mut width = 4;
        assert_eq!(run_ex_command("set sw=8", &mut b, &mut c, &mut width, None), VimEvent::IndentWidthChanged(8));
        assert_eq!(width, 8);
    }

    #[test]
    fn set_to_the_same_width_reports_no_change() {
        let (mut b, mut c) = cmd("hi");
        let mut width = 4;
        assert_eq!(run_ex_command("set sw=4", &mut b, &mut c, &mut width, None), VimEvent::None);
        assert_eq!(width, 4);
    }

    #[test]
    fn set_with_a_zero_or_unparsable_width_is_ignored() {
        let (mut b, mut c) = cmd("hi");
        let mut width = 4;
        assert_eq!(run_ex_command("set sw=0", &mut b, &mut c, &mut width, None), VimEvent::None);
        assert_eq!(width, 4);
        assert_eq!(run_ex_command("set sw=nope", &mut b, &mut c, &mut width, None), VimEvent::None);
        assert_eq!(width, 4);
    }

    #[test]
    fn set_with_an_unrecognized_option_is_ignored() {
        let (mut b, mut c) = cmd("hi");
        let mut width = 4;
        assert_eq!(run_ex_command("set number", &mut b, &mut c, &mut width, None), VimEvent::None);
        assert_eq!(width, 4);
    }

    #[test]
    fn bare_set_with_no_arguments_is_a_no_op() {
        let (mut b, mut c) = cmd("hi");
        let mut width = 4;
        assert_eq!(run_ex_command("set", &mut b, &mut c, &mut width, None), VimEvent::None);
        assert_eq!(width, 4);
    }
}
