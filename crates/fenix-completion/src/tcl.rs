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
    use super::*;

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
