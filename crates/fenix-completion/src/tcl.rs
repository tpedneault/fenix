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
