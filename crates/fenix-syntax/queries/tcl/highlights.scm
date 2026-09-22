(comment) @spell @comment

(command name: (simple_word) @function)

"proc" @keyword.function @keyword

; A proc's own name reads as a function, the same as a call to it --
; capturing it as a variable made every definition in a file render as
; plain body text, which is most of why Tcl looked flat.
(procedure
  name: (_) @function
)

(set (id) @variable)

(argument
  name: (_) @variable.parameter @variable
)

((simple_word) @variable.builtin @variable
               (#any-of? @variable.builtin
                "argc"
                "argv"
                "argv0"
                "auto_path"
                "env"
                "errorCode"
                "errorInfo"
                "tcl_interactive"
                "tcl_library"
                "tcl_nonwordchars"
                "tcl_patchLevel"
                "tcl_pkgPath"
                "tcl_platform"
                "tcl_precision"
                "tcl_rcFileName"
                "tcl_traceCompile"
                "tcl_traceExec"
                "tcl_wordchars"
                "tcl_version"))


"expr" @function.builtin @function

; Highlight switch arguments as string
(command
    name: (simple_word) @keyword
    arguments:
        (word_list
            (braced_word
                (command
                    name: (simple_word) @string)))
    (#eq? @keyword "switch"))

(command
  name: (simple_word) @function.builtin @function
  (#any-of? @function.builtin
   "cd"
   "exec"
   "exit"
   "incr"
   "info"
   "join"
   "puts"
   "regexp"
   "regsub"
   "split"
   "subst"
   "trace"
   "source"))

; Highlight unset and variable arguments as variables
(command
    name: (simple_word) @keyword
    arguments: (word_list) @variable
    (#any-of? @keyword
        "unset"
        "variable"))

(command name: (simple_word) @keyword
         (#any-of? @keyword
          "append"
          "break"
          "catch"
          "continue"
          "default"
          "dict"
          "error"
          "eval"
          "global"
          "lappend"
          "lassign"
          "lindex"
          "linsert"
          "list"
          "llength"
          "lmap"
          "lrange"
          "lrepeat"
          "lreplace"
          "lreverse"
          "lsearch"
          "lset"
          "lsort"
          "package"
          "return"
          "trap"
          "throw"))

[
 "catch"
 "error"
 "global"
 "namespace"
 "on"
 "set"
 "try"
 "finally"
 ] @keyword

(unpack) @operator

[
 "while"
 "foreach"
 ; "for"
 ] @repeat @keyword

[
 "if"
 "else"
 "elseif"
 ] @conditional @keyword

[
 "**"
 "/" "*" "%" "+" "-"
 "<<" ">>"
 ">" "<" ">=" "<="
 "==" "!="
 "eq" "ne"
 "in" "ni"
 "&"
 "^"
 "|"
 "&&"
 "||"
 ; `expr {$verbose ? 1 : 0}` -- the ternary's own two tokens, which
 ; were the only operators in an expression left uncolored.
 "?"
 ":"
 "!"
 "~"
 ] @operator

(variable_substitution) @variable

; A fully-qualified name passed as an *argument* (`lappend
; ::build_flags $x`, `namespace eval ::app {...}`) is a global variable
; or a namespace, and gets no capture of its own otherwise -- so those
; names rendered as plain body text. Scoped to `word_list` on purpose:
; a `::`-qualified word in *command* position (`::myns::greet arg`) is
; a call, and must keep the function color that rule gives it.
((word_list (simple_word) @variable)
            (#match? @variable "^::"))
(quoted_word) @string
(escaped_character) @string.escape

[
 "{" "}"
 "[" "]"
 ";"
 ] @punctuation.bracket @punctuation.delimiter

(number) @number

((simple_word) @number
               (#match? @number
                   "^[0-9]+$|^[+-]?[0-9]+$"))


((simple_word) @boolean
               (#any-of? @boolean "true" "false"))

; -- Ensemble subcommands -----------------------------------------------
;
; `dict keys $d`, `string compare $a $b`: the word right after an
; ensemble command's own name is really the operative part of the call
; -- the same idea `SPC c s`'s subcommand completion already acts on
; (`fenix_completion::tcl::subcommands`) -- so it reads as plain body
; text otherwise, the same gap a bare command name had before the
; `(command name: (_) @function)` rule at the top of this file.
;
; A `simple_word` grammar can't check *which* words are real
; subcommands the way the generated signature table can (that's a
; predicate no `#match?`/`#any-of?` regex expresses), so this only
; restricts by the ensemble's own name -- accepting the same
; imprecision the bare `@function` rule already does for command names
; in general (a typo'd subcommand still gets the color a real one
; would). The one real ambiguity is handled explicitly: `after` is
; also called as `after ms ?script ...?`, where the first word is a
; plain delay in milliseconds, not one of its three real subcommands
; (`cancel`/`idle`/`info`) -- restricted below so a delay never reads
; as though it were a subcommand.
;
; Reuses whatever capture the ensemble's own name already has elsewhere
; in this file (`@keyword` for "dict", `@function` for the rest, via
; the generic command-name rule below) rather than inventing a new one
; for it here -- both patterns capturing the identical node/range with
; the identical name is a no-op for `resolve_overlaps`, so there's
; nothing to keep in sync if that grouping ever changes. The
; subcommand word itself is always fresh territory (nothing else
; captures it), so `@function.builtin` there is a plain, unconflicted
; addition -- same color a builtin command name gets, which is the
; point.

(command
  name: (simple_word) @keyword
  arguments: (word_list . (simple_word) @function.builtin)
  (#eq? @keyword "dict"))

(command
  name: (simple_word) @function
  arguments: (word_list . (simple_word) @function.builtin)
  (#any-of? @function
   "array" "binary" "chan" "clock" "encoding"
   "file" "info" "interp" "package" "string" "trace"))

(command
  name: (simple_word) @function
  arguments: (word_list . (simple_word) @function.builtin)
  (#eq? @function "after")
  (#any-of? @function.builtin "cancel" "idle" "info"))

; `namespace` is its own grammar rule (not a plain `command`), so it
; needs no name-based restriction at all -- every node of this kind
; already means what it says.
(namespace
  (word_list . (simple_word) @function.builtin))

; A handful of ensembles nest a second ensemble one level down --
; `string is alnum`, `binary encode hex`, `info class methods`, `trace
; add variable` -- confirmed against the generated table (`tools/
; gen_tcl_signatures.tcl`'s own depth-3 entries), not guessed. The
; second word is real territory too (never captured above, since the
; first-level rules only reach the very first `word_list` child), so
; `@function.builtin.*`'s differing suffix exists purely so it isn't
; the *same* capture name as the gating first word within one pattern
; -- `#eq?`/`#any-of?` apply to every node under a given capture name,
; and the two words hold different text. Any name starting with
; `function.` still resolves to the same color (`theme::syntax_color`
; matches on the segment before the first `.`).

(command
  name: (simple_word) @function
  arguments: (word_list . (simple_word) @function.builtin . (simple_word) @function.builtin.class)
  (#eq? @function "string")
  (#eq? @function.builtin "is"))

(command
  name: (simple_word) @function
  arguments: (word_list . (simple_word) @function.builtin . (simple_word) @function.builtin.format)
  (#eq? @function "binary")
  (#any-of? @function.builtin "encode" "decode"))

(command
  name: (simple_word) @function
  arguments: (word_list . (simple_word) @function.builtin . (simple_word) @function.builtin.method)
  (#eq? @function "info")
  (#any-of? @function.builtin "class" "object"))

(command
  name: (simple_word) @function
  arguments: (word_list . (simple_word) @function.builtin . (simple_word) @function.builtin.type)
  (#eq? @function "trace")
  (#any-of? @function.builtin "add" "info" "remove"))


; after apply array auto_execok auto_import auto_load auto_mkindex auto_qualify
; auto_reset bgerror binary chan clock close coroutine dde encoding eof fblocked
; fconfigure fcopy file fileevent filename flush format gets glob history http
; interp load mathfunc mathop memory msgcat my next nextto open parray pid
; pkg::create pkg_mkIndex platform platform::shell pwd re_syntax read refchan
; registry rename safe scan seek self socket source string tailcall tcl::prefix
; tcl_endOfWord tcl_findLibrary tcl_startOfNextWord tcl_startOfPreviousWord
; tcl_wordBreakAfter tcl_wordBreakBefore tcltest tell time timerate tm
; transchan unknown unload update uplevel upvar vwait yield yieldto zlib
