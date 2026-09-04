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

; after apply array auto_execok auto_import auto_load auto_mkindex auto_qualify
; auto_reset bgerror binary chan clock close coroutine dde encoding eof fblocked
; fconfigure fcopy file fileevent filename flush format gets glob history http
; interp load mathfunc mathop memory msgcat my next nextto open parray pid
; pkg::create pkg_mkIndex platform platform::shell pwd re_syntax read refchan
; registry rename safe scan seek self socket source string tailcall tcl::prefix
; tcl_endOfWord tcl_findLibrary tcl_startOfNextWord tcl_startOfPreviousWord
; tcl_wordBreakAfter tcl_wordBreakBefore tcltest tell time timerate tm
; transchan unknown unload update uplevel upvar vwait yield yieldto zlib
