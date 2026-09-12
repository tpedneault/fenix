use crate::tcl_signatures;

/// Every `proc` name defined in one Tcl source's own text.
///
/// The point is highlighting a file that isn't part of an indexed
/// project: a call to a proc defined twenty lines up is the most
/// ordinary thing in a Tcl script, and without this it renders as plain
/// body text, because `ctags` -- the only other source of user-defined
/// names -- has nothing to index when there's no project root, and
/// hasn't been re-run since the proc was typed even when there is.
///
/// A deliberately literal scan rather than a parse: `proc` is only a
/// definition at the start of a command, which after leading whitespace
/// is the start of a line or just past a `{`/`[`/`;`, and that is
/// cheap enough to redo whenever the buffer changes. A name inside a
/// string or a comment can slip through; the cost of that is one extra
/// word being colored as a command, which is the same thing that
/// happens for any real proc defined in another file.
pub fn procs_defined_in(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in source.lines() {
        // Commands can be chained on one line (`set a 1 ; proc b {} {}`),
        // and a proc is routinely nested inside a `namespace eval {`
        // block, so every command boundary on the line is a candidate
        // start -- not just the line's own.
        for piece in line.split(['{', '[', ';', '}', ']']) {
            let piece = piece.trim_start();
            let Some(rest) = piece.strip_prefix("proc") else { continue };
            if !rest.starts_with(char::is_whitespace) {
                continue;
            }
            let Some(name) = rest.split_whitespace().next() else { continue };
            // A `$`-substituted or bracketed name isn't a literal
            // definition this can resolve, and a comment's `proc` is
            // not a definition at all.
            if name.starts_with('$') || name.starts_with('#') {
                continue;
            }
            names.push(name.to_string());
        }
    }
    names
}

/// The usage line Tcl itself reports for an exact command path --
/// `signature("string compare")` is `string compare ?-nocase? ?-length
/// int? string1 string2`. `None` for a path the table doesn't know
/// (a user proc, a misspelling, an option word). Paths are the
/// space-joined leading words of a command exactly as typed, with a
/// leading `::` on the command name tolerated since `::string` is the
/// same command.
pub fn signature(path: &str) -> Option<&'static str> {
    let path = path.strip_prefix("::").unwrap_or(path);
    tcl_signatures::SIGNATURES.binary_search_by(|(candidate, _)| candidate.cmp(&path)).ok().map(|i| tcl_signatures::SIGNATURES[i].1)
}

/// The direct subcommands of `path` with their own usage lines --
/// `subcommands("string")` starts `("bytelength", "string bytelength
/// string"), ("cat", ...)`, `subcommands("string is")` lists the
/// character classes, and `subcommands("puts")` is empty because `puts`
/// isn't an ensemble. Only one level down: `subcommands("binary")`
/// yields `decode`/`encode`/`format`/`scan`, not `encode hex`. Comes
/// back in table (sorted) order.
pub fn subcommands(path: &str) -> Vec<(&'static str, &'static str)> {
    let path = path.strip_prefix("::").unwrap_or(path);
    let prefix = format!("{path} ");
    let start = tcl_signatures::SIGNATURES.partition_point(|(candidate, _)| *candidate < prefix.as_str());
    tcl_signatures::SIGNATURES[start..]
        .iter()
        .take_while(|(candidate, _)| candidate.starts_with(&prefix))
        .filter_map(|(candidate, usage)| {
            let rest = &candidate[prefix.len()..];
            (!rest.contains(' ')).then_some((rest, *usage))
        })
        .collect()
}

/// A character that starts a new Tcl command (or a script body a command
/// can start inside): the segment after the last of these is the command
/// the cursor is in. Mirrors `procs_defined_in`'s boundary set -- `{`
/// because `if {..} {string ` and `foreach x $l {string ` both start a
/// command right after the brace, `[` for command substitution.
fn is_command_boundary(c: char) -> bool {
    matches!(c, '[' | ';' | '{' | '}' | ']' | '\n')
}

/// The leading words of the command the cursor is in, from `text` --
/// everything on the cursor's line *before* the point of interest
/// (the completion prefix's start, or the hovered word's start). The
/// point has to sit at a word boundary: `"set x [string "` is
/// `Some(["string"])`, `"  string is "` is `Some(["string", "is"])`,
/// and `"set x ["` or `""` is `Some([])` (the point is the command's
/// first word), but `"strin"` (mid-word) and `"puts [string"` (the
/// cursor is *on* `string`, not after it) are both `None`. Also `None`
/// when any leading word isn't a plain command word (`$obj string ` --
/// a substitution, not a command the table could know), so callers
/// never look up a nonsense path.
pub fn command_words_before(text: &str) -> Option<Vec<&str>> {
    if !text.is_empty() && !text.ends_with(|c: char| c.is_whitespace() || is_command_boundary(c)) {
        return None;
    }
    let segment = text.rsplit(is_command_boundary).next().unwrap_or(text);
    let words: Vec<&str> = segment.split_whitespace().collect();
    if words.iter().any(|w| w.starts_with('$') || w.starts_with('"') || w.starts_with('#')) {
        return None;
    }
    Some(words)
}

/// The most specific known command for a cursor sitting on `word` with
/// `before` (see `command_words_before`) leading up to it, as `(path,
/// usage)`: `string compare` when the cursor is on `compare`, and still
/// `string compare` when it's on the `-nocase` or `$s` after that (the
/// option isn't a path, so it falls back to the command it's an
/// argument of). `None` when no prefix of the words is a known command
/// at all -- a user proc, a variable, a bare word.
pub fn signature_at(before: &[&str], word: &str) -> Option<(String, &'static str)> {
    let mut words: Vec<&str> = before.to_vec();
    words.push(word);
    while !words.is_empty() {
        let path = words.join(" ");
        if let Some(usage) = signature(&path) {
            return Some((path, usage));
        }
        words.pop();
    }
    None
}

/// What `K` shows for a built-in with no language server to ask: the
/// usage line, plus -- for an ensemble -- its subcommands, wrapped so
/// `string`'s twenty-three fit in a few popup rows rather than one
/// line that runs off the window. `None` for a word that isn't (an
/// argument of) a known built-in.
pub fn hover_text(before: &[&str], word: &str) -> Option<String> {
    const WRAP_AT: usize = 72;
    let (path, usage) = signature_at(before, word)?;
    let mut text = usage.to_string();
    let subs = subcommands(&path);
    if !subs.is_empty() {
        let mut line = String::from("subcommands: ");
        for (i, (name, _)) in subs.iter().enumerate() {
            let piece = if i + 1 == subs.len() { name.to_string() } else { format!("{name}, ") };
            if line.len() + piece.trim_end().len() > WRAP_AT {
                text.push('\n');
                text.push_str(line.trim_end());
                line = String::from("  ");
            }
            line.push_str(&piece);
        }
        text.push('\n');
        text.push_str(line.trim_end());
    }
    Some(text)
}

/// The word at char index `col` in `line`, widened over identifier
/// chars in both directions the way Vim's `iw` would -- the hover
/// target for `K` in Normal mode, where the cursor sits *on* a word
/// rather than after it. `(start, end)` char offsets; `None` on
/// whitespace or punctuation.
pub fn word_at(line: &str, col: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == ':' || c == '-';
    if !chars.get(col).copied().is_some_and(is_word) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col + 1;
    while end < chars.len() && is_word(chars[end]) {
        end += 1;
    }
    Some((start, end))
}

/// Tcl 8.6 built-in command names, for keyword-source completion (the only
/// source available for a language with no LSP server). Deliberately
/// broader than `fenix-syntax`'s own Tcl `highlights.scm` "keyword"
/// capture set -- that one exists to pick highlight colors for a narrow
/// control-flow-ish node-type subset, while completion's job is to offer
/// every commonly-typed builtin.
///
/// Generated from a real `tclsh 8.6` via `puts [lsort [info commands]]`
/// (verified against actual ground truth, not typed from memory), with
/// Tcl's own internal autoloading/introspection machinery dropped
/// (`auto_execok`, `auto_import`, `auto_load`, `auto_load_index`,
/// `auto_qualify`, `tclLog`, `unknown` -- none of these are commands a
/// user ever types in ordinary Tcl code), plus `else`/`elseif`/`then`
/// added by hand: real Tcl syntax words accepted as arguments to `if`
/// (not separate commands, so `info commands` correctly omits them), but
/// exactly the kind of thing a user typing `el` inside an `if` expects to
/// complete.
pub const KEYWORDS: &[&str] = &[
    "after", "append", "apply", "array", "binary", "break", "case", "catch", "cd", "chan", "clock", "close",
    "concat", "continue", "coroutine", "dict", "default", "else", "elseif", "encoding", "eof", "error", "eval",
    "exec", "exit", "expr", "fblocked", "fconfigure", "fcopy", "file", "fileevent", "flush", "for", "foreach",
    "format", "gets", "glob", "global", "history", "if", "incr", "info", "interp", "join", "lappend", "lassign",
    "lindex", "linsert", "list", "llength", "lmap", "load", "lrange", "lrepeat", "lreplace", "lreverse", "lsearch",
    "lset", "lsort", "namespace", "open", "package", "pid", "proc", "puts", "pwd", "read", "regexp", "regsub",
    "rename", "return", "scan", "seek", "set", "socket", "source", "split", "string", "subst", "switch",
    "tailcall", "tell", "then", "throw", "time", "trace", "try", "unload", "unset", "update", "uplevel", "upvar",
    "variable", "vwait", "while", "yield", "yieldto", "zlib",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// `signature`'s binary search and `subcommands`' partition point
    /// both assume the generated table is sorted by path -- the
    /// generator's `lsort -unique -index 0` guarantees it, and this
    /// catches a hand edit that breaks it.
    #[test]
    fn the_signature_table_is_sorted_by_path_with_no_duplicates() {
        for pair in tcl_signatures::SIGNATURES.windows(2) {
            assert!(pair[0].0 < pair[1].0, "{:?} should sort before {:?}", pair[0].0, pair[1].0);
        }
    }

    #[test]
    fn signature_returns_tcls_own_usage_line_for_a_known_path() {
        assert_eq!(signature("string compare"), Some("string compare ?-nocase? ?-length int? string1 string2"));
        assert_eq!(signature("lappend"), Some("lappend varName ?value ...?"));
        assert_eq!(signature("::string"), signature("string"));
        assert_eq!(signature("my_proc"), None);
        assert_eq!(signature("string compare -nocase"), None);
    }

    #[test]
    fn subcommands_lists_exactly_one_level_below_the_path() {
        let string: Vec<&str> = subcommands("string").into_iter().map(|(name, _)| name).collect();
        assert!(string.contains(&"compare") && string.contains(&"is") && string.contains(&"map"), "{string:?}");
        let binary: Vec<&str> = subcommands("binary").into_iter().map(|(name, _)| name).collect();
        assert_eq!(binary, ["decode", "encode", "format", "scan"]);
        let encode: Vec<&str> = subcommands("binary encode").into_iter().map(|(name, _)| name).collect();
        assert_eq!(encode, ["base64", "hex", "uuencode"]);
        assert!(subcommands("string is").iter().any(|(name, _)| *name == "alnum"));
        assert!(subcommands("puts").is_empty());
        assert!(subcommands("nonsense").is_empty());
    }

    #[test]
    fn each_subcommand_carries_its_own_usage_line() {
        let (name, usage) = subcommands("dict").into_iter().find(|(name, _)| *name == "set").unwrap();
        assert_eq!(name, "set");
        assert_eq!(usage, "dict set dictVarName key ?key ...? value");
    }

    #[test]
    fn command_words_before_reads_back_to_the_start_of_the_current_command() {
        assert_eq!(command_words_before("string "), Some(vec!["string"]));
        assert_eq!(command_words_before("    string is "), Some(vec!["string", "is"]));
        assert_eq!(command_words_before("set x [string "), Some(vec!["string"]));
        assert_eq!(command_words_before("if {$x} {string "), Some(vec!["string"]));
        assert_eq!(command_words_before("set a 1; dict "), Some(vec!["dict"]));
    }

    #[test]
    fn command_words_before_is_none_mid_word_or_after_a_substitution() {
        assert_eq!(command_words_before("strin"), None, "the cursor is still typing the command name");
        assert_eq!(command_words_before("puts [string"), None);
        assert_eq!(command_words_before("$obj string "), None);
        assert_eq!(command_words_before("# string "), None);
        assert_eq!(command_words_before("string map {a b} "), Some(vec![]), "the braced word ended the leading-words run");
    }

    #[test]
    fn command_words_before_is_empty_when_the_point_is_the_commands_first_word() {
        assert_eq!(command_words_before(""), Some(vec![]));
        assert_eq!(command_words_before("   "), Some(vec![]));
        assert_eq!(command_words_before("set x ["), Some(vec![]));
        assert_eq!(command_words_before("set a 1;"), Some(vec![]));
    }

    #[test]
    fn signature_at_falls_back_to_the_longest_known_prefix() {
        let usage = |path: &str| signature(path).unwrap();
        assert_eq!(signature_at(&["string"], "compare"), Some(("string compare".to_string(), usage("string compare"))));
        assert_eq!(signature_at(&["string", "compare"], "-nocase"), Some(("string compare".to_string(), usage("string compare"))));
        assert_eq!(signature_at(&[], "string"), Some(("string".to_string(), usage("string"))));
        assert_eq!(signature_at(&[], "my_proc"), None);
        assert_eq!(signature_at(&["my_proc"], "string"), None, "an argument named like a command is not that command");
    }

    #[test]
    fn hover_text_is_the_usage_line_plus_a_wrapped_subcommand_list_for_an_ensemble() {
        let text = hover_text(&[], "string").unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("string subcommand ?arg ...?"));
        let rest: Vec<&str> = lines.collect();
        assert!(rest[0].starts_with("subcommands: bytelength, cat, compare,"), "{rest:?}");
        assert!(rest.len() >= 2, "twenty-three subcommands need more than one 72-column line: {rest:?}");
        assert!(rest.iter().all(|line| line.len() <= 72), "{rest:?}");
        assert!(rest.last().unwrap().ends_with("wordstart"), "{rest:?}");
        assert!(!text.contains(", \n"), "no line should end in a dangling comma-space");
    }

    #[test]
    fn hover_text_is_just_the_usage_line_for_a_plain_command() {
        assert_eq!(hover_text(&[], "lappend").as_deref(), Some("lappend varName ?value ...?"));
        assert_eq!(hover_text(&["string"], "compare").as_deref(), Some("string compare ?-nocase? ?-length int? string1 string2"));
        assert_eq!(hover_text(&[], "my_proc"), None);
    }

    #[test]
    fn word_at_widens_over_the_word_under_the_cursor() {
        assert_eq!(word_at("string compare $a", 9), Some((7, 14)));
        assert_eq!(word_at("string compare $a", 0), Some((0, 6)));
        assert_eq!(word_at("string compare $a", 6), None);
        assert_eq!(word_at("x -nocase", 3), Some((2, 9)));
        assert_eq!(word_at("ab", 5), None);
    }

    #[test]
    fn finds_procs_defined_in_the_file_itself() {
        // Including one nested in a `namespace eval` block and one
        // chained after another command on the same line -- both are
        // ordinary Tcl, and both are missed by a line-start-only scan.
        let source = "proc alpha {a} {
  return $a
}

namespace eval ::app { proc beta {} {} }
set x 1 ; proc gamma {} {}
";
        let mut found = procs_defined_in(source);
        found.sort();
        assert_eq!(found, vec!["alpha", "beta", "gamma"].into_iter().map(String::from).collect::<Vec<_>>());
    }

    #[test]
    fn a_word_merely_starting_with_proc_is_not_a_definition() {
        assert!(procs_defined_in("procedure_name foo
").is_empty());
        assert!(procs_defined_in("set procs 3
").is_empty());
    }

    #[test]
    fn a_computed_proc_name_is_skipped_rather_than_guessed_at() {
        assert!(procs_defined_in("proc $name {} {}
").is_empty());
    }

    #[test]
    fn common_builtins_are_present() {
        for expected in ["set", "proc", "if", "foreach", "expr", "string", "dict", "namespace", "variable"] {
            assert!(KEYWORDS.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn internal_autoloading_machinery_is_excluded() {
        for excluded in ["auto_execok", "auto_import", "auto_load", "auto_load_index", "auto_qualify", "tclLog", "unknown"]
        {
            assert!(!KEYWORDS.contains(&excluded), "should not offer internal command {excluded}");
        }
    }

    #[test]
    fn has_no_duplicates() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), KEYWORDS.len());
    }
}
