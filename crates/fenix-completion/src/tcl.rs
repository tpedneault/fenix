use crate::tcl_signatures;

/// One `proc` definition found in a Tcl source's own text: its name as
/// written (`greet`, or `::util::greet` for a qualified definition)
/// and its argument list exactly as written (`{name {greeting hello}
/// args}`, braces included -- the same shape ctags' `signature:` field
/// has, so `proc_signature` reads both).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcDef {
    pub name: String,
    pub args: String,
    /// The namespace the definition sits in, from enclosing `namespace
    /// eval` blocks -- `util` for a `proc greet` inside `namespace eval
    /// util {..}`, empty at the top level. What `qualified_name` joins
    /// with `name` unless the name is already qualified.
    pub namespace: String,
}

impl ProcDef {
    /// `util::greet` -- the name the ctags index would list this under
    /// (never a leading `::`, same convention as `ctags::TagEntry`).
    pub fn qualified_name(&self) -> String {
        if self.name.contains("::") || self.namespace.is_empty() {
            self.name.trim_start_matches("::").to_string()
        } else {
            format!("{}::{}", self.namespace, self.name)
        }
    }
}

/// Every `proc` name defined in one Tcl source's own text.
///
/// The point is highlighting a file that isn't part of an indexed
/// project: a call to a proc defined twenty lines up is the most
/// ordinary thing in a Tcl script, and without this it renders as plain
/// body text, because `ctags` -- the only other source of user-defined
/// names -- has nothing to index when there's no project root, and
/// hasn't been re-run since the proc was typed even when there is.
/// See `proc_definitions_in` for the scan itself.
pub fn procs_defined_in(source: &str) -> Vec<String> {
    proc_definitions_in(source).into_iter().map(|def| def.name).collect()
}

/// Every `proc` definition in `source`, with its argument list -- what
/// `procs_defined_in` is built on, and what gives a proc defined in the
/// buffer being edited a signature (`proc_signature`) before ctags has
/// been re-run.
///
/// A deliberately literal scan rather than a parse: `proc` is only a
/// definition at the start of a command, which after leading whitespace
/// is the start of a line or just past a `{`/`[`/`;`, and that is
/// cheap enough to redo whenever the buffer changes. The argument list
/// is the one word after the name, read with brace matching so a
/// multi-line `{a\n b}` survives. A `proc` inside a string or a
/// comment can slip through; the cost of that is one extra word being
/// colored as a command, which is the same thing that happens for any
/// real proc defined in another file. Namespaces are tracked by
/// matching the braces of `namespace eval NAME {` blocks, so a proc
/// nested in one is reported under it.
pub fn proc_definitions_in(source: &str) -> Vec<ProcDef> {
    let chars: Vec<char> = source.chars().collect();
    let mut defs = Vec::new();
    // Open `namespace eval` blocks: (namespace name, brace depth its
    // body opened at).
    let mut namespaces: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    let mut at_command_start = true;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if at_command_start && !c.is_whitespace() {
            at_command_start = false;
            if let Some((word, next)) = read_word(&chars, i) {
                if word == "proc" {
                    if let Some((name, next)) = read_word(&chars, next) {
                        if !name.starts_with('$') && !name.starts_with('[') {
                            let (args, next) = read_word(&chars, next).unwrap_or((String::new(), next));
                            let namespace = namespaces.last().map(|(ns, _)| ns.clone()).unwrap_or_default();
                            defs.push(ProcDef { name, args, namespace });
                            i = next;
                            continue;
                        }
                    }
                } else if word == "namespace" {
                    if let Some((sub, next)) = read_word(&chars, next) {
                        if sub == "eval" {
                            if let Some((name, next)) = read_word(&chars, next) {
                                // The body's opening brace is the next
                                // non-blank char; its depth after opening
                                // is what closes the block.
                                let mut j = next;
                                while j < chars.len() && chars[j].is_whitespace() { j += 1; }
                                if chars.get(j) == Some(&'{') {
                                    let name = name.trim_start_matches("::").to_string();
                                    let full = match namespaces.last() {
                                        Some((outer, _)) if !outer.is_empty() => format!("{outer}::{name}"),
                                        _ => name,
                                    };
                                    namespaces.push((full, depth + 1));
                                }
                            }
                        }
                    }
                }
            }
        }
        match c {
            '{' | '[' => { depth += 1; at_command_start = true; }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                if namespaces.last().is_some_and(|(_, opened)| *opened > depth) {
                    namespaces.pop();
                }
                at_command_start = true;
            }
            ';' | '\n' => at_command_start = true,
            '#' if at_command_start => {
                // A comment runs to the end of the line.
                while i < chars.len() && chars[i] != '\n' { i += 1; }
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    defs
}

/// The Tcl word starting at `from` (after any whitespace), and the index
/// just past it: a braced word with its braces (nesting respected), a
/// quoted word with its quotes, or a bare word up to the next blank or
/// command-ending char. `None` at the end of the text or when the word
/// would be empty.
fn read_word(chars: &[char], from: usize) -> Option<(String, usize)> {
    let mut i = from;
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') { i += 1; }
    let start = i;
    match chars.get(i)? {
        '{' => {
            let mut level = 0usize;
            while i < chars.len() {
                match chars[i] {
                    '{' => level += 1,
                    '}' => { level -= 1; if level == 0 { i += 1; break; } }
                    _ => {}
                }
                i += 1;
            }
        }
        '"' => {
            i += 1;
            while i < chars.len() && chars[i] != '"' { i += 1; }
            i = (i + 1).min(chars.len());
        }
        _ => {
            while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], ';' | '{' | '}' | '[' | ']') { i += 1; }
        }
    }
    (i > start).then(|| (chars[start..i].iter().collect(), i))
}

/// The words of a Tcl list as written: `{name {greeting hello} args}`
/// or `name {greeting hello} args` gives `name`, `{greeting hello}`,
/// `args`. Outer braces are stripped once; inner braced words keep
/// theirs so `proc_signature` can tell a `{name default}` pair from a
/// plain name.
pub fn tcl_list_words(list: &str) -> Vec<String> {
    let list = list.trim();
    let inner = list.strip_prefix('{').and_then(|s| s.strip_suffix('}')).unwrap_or(list);
    let chars: Vec<char> = inner.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    loop {
        while i < chars.len() && chars[i].is_whitespace() { i += 1; }
        let Some((word, next)) = read_word(&chars, i) else { break };
        words.push(word);
        i = next;
    }
    words
}

/// A user proc's usage line in Tcl's own convention, from its argument
/// list as written: `proc_signature("util::greet", "{name {greeting
/// hello} args}")` is `util::greet name ?greeting? ?arg ...?` -- a
/// `{name default}` pair is optional, a trailing `args` is variadic.
/// The same shape `signature` gives built-ins, so hover and completion
/// treat both alike.
pub fn proc_signature(name: &str, args: &str) -> String {
    let words = tcl_list_words(args);
    let mut out = name.to_string();
    for (i, word) in words.iter().enumerate() {
        out.push(' ');
        if word.starts_with('{') {
            let pair = tcl_list_words(word);
            match pair.first() {
                Some(name) if pair.len() >= 2 => { out.push('?'); out.push_str(name); out.push('?'); }
                Some(name) => out.push_str(name),
                None => out.push_str("{}"),
            }
        } else if word == "args" && i + 1 == words.len() {
            out.push_str("?arg ...?");
        } else {
            out.push_str(word);
        }
    }
    out
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

/// The entries exactly one word below `path`, as `(word, usage)` --
/// subcommands and `-flag` options alike, in table (sorted) order.
fn children(path: &str) -> impl Iterator<Item = (&'static str, &'static str)> {
    let path = path.strip_prefix("::").unwrap_or(path);
    let prefix = format!("{path} ");
    let start = tcl_signatures::SIGNATURES.partition_point(|(candidate, _)| *candidate < prefix.as_str());
    let width = prefix.len();
    tcl_signatures::SIGNATURES[start..]
        .iter()
        .take_while(move |(candidate, _)| candidate.starts_with(&prefix))
        .filter_map(move |(candidate, usage)| {
            let rest = &candidate[width..];
            (!rest.contains(' ')).then_some((rest, *usage))
        })
}

/// The direct subcommands of `path` with their own usage lines --
/// `subcommands("string")` starts `("bytelength", "string bytelength
/// string"), ("cat", ...)`, `subcommands("string is")` lists the
/// character classes, and `subcommands("puts")` is empty because `puts`
/// isn't an ensemble. Only one level down: `subcommands("binary")`
/// yields `decode`/`encode`/`format`/`scan`, not `encode hex`. Never
/// includes a `-flag` -- those are `options`.
pub fn subcommands(path: &str) -> Vec<(&'static str, &'static str)> {
    children(path).filter(|(word, _)| !word.starts_with('-')).collect()
}

/// The `-flag` options `path` accepts, each paired with the command's
/// own usage line (the one place the flag is explained): `options(
/// "lsort")` is `-ascii`, `-command`, ... `-unique`; `options("string
/// compare")` is `-length` and `-nocase`; `options("set")` is empty.
pub fn options(path: &str) -> Vec<(&'static str, &'static str)> {
    children(path).filter(|(word, _)| word.starts_with('-')).collect()
}

/// The options for a cursor whose command's leading words are `words`
/// -- those of the longest prefix of `words` that has any, so `lsort
/// -unique -` still offers `lsort`'s (the `-unique` already typed
/// isn't a command with options of its own) and `string compare
/// -nocase -` offers `string compare`'s. Empty when no prefix is a
/// command with options.
pub fn options_at(words: &[&str]) -> Vec<(&'static str, &'static str)> {
    (1..=words.len())
        .rev()
        .map(|n| options(&words[..n].join(" ")))
        .find(|found| !found.is_empty())
        .unwrap_or_default()
}

/// The `-flag` being typed at the end of `text` (the cursor's line up
/// to the cursor), as `(char offset of the dash, the flag so far)`:
/// `"lsort -"` is `Some((6, "-"))`, `"lsort -uni"` is `Some((6,
/// "-uni"))`, `"regexp --"` is `Some((7, "--"))`. `None` when the last
/// word doesn't start with a dash, or the dash is glued to something
/// (`end-1`, `$a-`) rather than starting a word.
pub fn option_prefix(text: &str) -> Option<(usize, &str)> {
    let start = text.rfind(|c: char| c.is_whitespace() || is_command_boundary(c)).map_or(0, |i| i + 1);
    let word = &text[start..];
    if !word.starts_with('-') || !word.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        return None;
    }
    Some((text[..start].chars().count(), word))
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
        assert_eq!(signature("string compare -nocase"), signature("string compare"), "an option entry carries its command's usage line");
        assert_eq!(signature("string compare -bogus"), None);
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
        assert!(subcommands("lsort").is_empty(), "flags are options, not subcommands");
    }

    #[test]
    fn options_lists_a_commands_flags_with_its_own_usage_line() {
        let lsort: Vec<&str> = options("lsort").into_iter().map(|(name, _)| name).collect();
        assert!(lsort.contains(&"-unique") && lsort.contains(&"-dictionary"), "{lsort:?}");
        assert!(lsort.iter().all(|name| name.starts_with('-')), "{lsort:?}");
        let compare: Vec<&str> = options("string compare").into_iter().map(|(name, _)| name).collect();
        assert_eq!(compare, ["-length", "-nocase"]);
        assert_eq!(options("string compare")[0].1, signature("string compare").unwrap());
        assert!(options("regexp").iter().any(|(name, _)| *name == "--"));
        assert!(options("puts").iter().any(|(name, _)| *name == "-nonewline"), "a man-page override for a command the probe can't reach");
        assert!(options("set").is_empty());
        assert!(options("string").is_empty(), "the ensemble itself has no flags, its subcommands do");
    }

    #[test]
    fn options_at_uses_the_longest_prefix_that_has_options() {
        assert_eq!(options_at(&["lsort"]), options("lsort"));
        assert_eq!(options_at(&["lsort", "-unique"]), options("lsort"));
        assert_eq!(options_at(&["string", "compare", "-nocase"]), options("string compare"));
        assert!(options_at(&["set", "x"]).is_empty());
        assert!(options_at(&["string"]).is_empty());
        assert!(options_at(&[]).is_empty());
    }

    #[test]
    fn option_prefix_finds_a_flag_being_typed_at_the_end_of_the_line() {
        assert_eq!(option_prefix("lsort -"), Some((6, "-")));
        assert_eq!(option_prefix("lsort -uni"), Some((6, "-uni")));
        assert_eq!(option_prefix("regexp --"), Some((7, "--")));
        assert_eq!(option_prefix("set x [lsort -d"), Some((13, "-d")));
        assert_eq!(option_prefix("lsort"), None);
        assert_eq!(option_prefix("lsort "), None);
        assert_eq!(option_prefix("lindex $l end-"), None, "a dash glued to a word is arithmetic, not a flag");
        assert_eq!(option_prefix("expr {$a -"), Some((9, "-")), "the dash alone can't tell -- `options_at` rules this out by the words before it");
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
        assert_eq!(signature_at(&["string", "compare"], "-nocase"), Some(("string compare -nocase".to_string(), usage("string compare"))), "an option entry carries its command's usage line");
        assert_eq!(signature_at(&["string", "compare", "-nocase"], "$s"), Some(("string compare -nocase".to_string(), usage("string compare"))));
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
    fn proc_definitions_keep_the_argument_list_and_the_enclosing_namespace() {
        let source = "namespace eval util {\n    proc greet {name {greeting hello} args} {\n        puts \"$greeting $name\"\n    }\n    namespace eval inner { proc deep {x} {} }\n}\nproc plain {a b} {}\nproc ::qualified::name {} {}\nproc single x {}\n";
        let defs = proc_definitions_in(source);
        let summary: Vec<(String, String, String)> = defs.iter().map(|d| (d.qualified_name(), d.args.clone(), d.namespace.clone())).collect();
        assert_eq!(summary, vec![
            ("util::greet".to_string(), "{name {greeting hello} args}".to_string(), "util".to_string()),
            ("util::inner::deep".to_string(), "{x}".to_string(), "util::inner".to_string()),
            ("plain".to_string(), "{a b}".to_string(), String::new()),
            ("qualified::name".to_string(), "{}".to_string(), String::new()),
            ("single".to_string(), "x".to_string(), String::new()),
        ]);
    }

    #[test]
    fn a_multi_line_argument_list_is_read_whole() {
        let defs = proc_definitions_in("proc f {\n    a\n    {b 2}\n} {}\n");
        assert_eq!(defs[0].args, "{\n    a\n    {b 2}\n}");
        assert_eq!(proc_signature("f", &defs[0].args), "f a ?b?");
    }

    #[test]
    fn a_proc_in_a_comment_is_not_a_definition() {
        assert!(proc_definitions_in("# proc commented {} {}\n").is_empty());
        assert_eq!(proc_definitions_in("set x 1 ;# proc trailing {} {}\nproc real {} {}\n").len(), 1);
    }

    #[test]
    fn proc_signature_uses_tcls_own_optional_and_variadic_notation() {
        assert_eq!(proc_signature("util::greet", "{name {greeting hello} args}"), "util::greet name ?greeting? ?arg ...?");
        assert_eq!(proc_signature("plain", "{a b}"), "plain a b");
        assert_eq!(proc_signature("noargs", "{}"), "noargs");
        assert_eq!(proc_signature("single", "x"), "single x");
        assert_eq!(proc_signature("f", "{args}"), "f ?arg ...?");
        assert_eq!(proc_signature("f", "{args tail}"), "f args tail", "`args` is only variadic in last position");
        assert_eq!(proc_signature("f", "{{opt {a b}}}"), "f ?opt?", "a default containing spaces is still one pair");
    }

    #[test]
    fn tcl_list_words_splits_on_blanks_and_respects_braces() {
        assert_eq!(tcl_list_words("{a {b c} d}"), ["a", "{b c}", "d"]);
        assert_eq!(tcl_list_words("a {b c} d"), ["a", "{b c}", "d"]);
        assert_eq!(tcl_list_words("{}"), Vec::<String>::new());
        assert_eq!(tcl_list_words("  {x}  "), ["x"]);
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
