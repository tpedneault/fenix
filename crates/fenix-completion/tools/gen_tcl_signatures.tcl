#!/usr/bin/env tclsh
# Generates `src/tcl_signatures.rs` from a real Tcl interpreter.
#
#   cd crates/fenix-completion && tclsh tools/gen_tcl_signatures.tcl src/tcl_signatures.rs
#
# Two facts are extracted per command path, both straight from Tcl's
# own C-level argument checking rather than from anything typed by hand:
#
#   * its subcommands -- calling `cmd __probe` makes an ensemble (or a
#     C command with a subcommand table) list every valid one:
#       unknown or ambiguous subcommand "__probe": must be a, b, or c
#       bad option "__probe": must be a, b, or c
#     (`string is` says "bad class", `after` says "bad argument"; the
#     "must be" list is what matters, whichever noun precedes it);
#   * its synopsis -- calling it with no arguments, then (if that was a
#     valid call) with far too many, makes it report
#       wrong # args: should be "string compare ?-nocase? ?-length int? string1 string2"
#     which is the exact usage line the C implementation carries;
#   * its options -- calling it with a bogus `-__probe` flag (padded with
#     filler arguments before and after, since `lsearch` only parses
#     flags once it has a list and a pattern and `clock format` only
#     after its clock value) makes it report
#       bad option "-__probe": must be -all, -ascii, -bisect, ... or -subindices
#     Recorded as `path -flag` entries carrying the path's own synopsis.
#
# Probing is side-effect free by construction: every call either errors
# on arity before doing anything or is a read-only introspection --
# *except* for the commands in `UNSAFE`, which are never given any
# argument at all (`file mkdir __probe` would create a directory, `vwait
# __probe` would block forever, `tailcall` would escape the `catch`),
# and whose synopses come from `OVERRIDES` (copied from the Tcl 8.6 man
# pages) instead. `file` itself is still asked for its subcommand list
# (`file __probe` is a harmless "bad option"), which is why it's listed
# under `ENUMERATE_ONLY` rather than `UNSAFE`. Run it from a scratch
# directory anyway.

# Captured before this script defines anything, so its own procs never
# end up in the table.
set ROOTS [lsort [info commands]]

set UNSAFE {exec cd open source load unload socket vwait exit update tailcall yield yieldto}
set ENUMERATE_ONLY {file}
set NOT_COMMANDS {else elseif then}

# Synopses the probes can't produce: `UNSAFE` commands, and commands
# whose only arguments are optional/variadic so no arity ever fails.
# Copied from the Tcl 8.6 man pages.
array set OVERRIDES {
    "cd"               "cd ?dirName?"
    "exec"             "exec ?switches? arg ?arg ...?"
    "exit"             "exit ?returnCode?"
    "open"             "open fileName ?access? ?permissions?"
    "source"           "source ?-encoding encodingName? fileName"
    "load"             "load ?-global? ?-lazy? ?--? fileName ?packageName? ?interp?"
    "unload"           "unload ?switches? fileName ?packageName? ?interp?"
    "socket"           "socket ?-myaddr addr? ?-myport myport? ?-async? host port"
    "vwait"            "vwait varName"
    "update"           "update ?idletasks?"
    "tailcall"         "tailcall command ?arg ...?"
    "yield"            "yield ?value?"
    "yieldto"          "yieldto command ?arg ...?"
    "list"             "list ?arg ...?"
    "if"               "if expr1 ?then? body1 elseif expr2 ?then? body2 elseif ... ?else? ?bodyN?"
    "return"           "return ?option value ...? ?result?"
    "unset"            "unset ?-nocomplain? ?--? ?name name name ...?"
    "variable"         "variable ?name value...? name ?value?"
    "global"           "global ?varname ...?"
    "glob"             "glob ?switches? ?pattern ...?"
    "dict create"      "dict create ?key value ...?"
    "dict merge"       "dict merge ?dictionaryValue ...?"
    "string cat"       "string cat ?string1? ?string2...?"
    "namespace export" "namespace export ?-clear? ?pattern pattern ...?"
    "namespace forget" "namespace forget ?pattern pattern ...?"
    "namespace import" "namespace import ?-force? ?pattern pattern ...?"
    "package forget"   "package forget ?package package ...?"
    "file channels"    "file channels ?pattern?"
    "file separator"   "file separator ?name?"
    "file tempfile"    "file tempfile ?nameVar? ?template?"
    "concat"           "concat ?arg ...?"
    "file delete"      "file delete ?-force? ?--? ?pathname ...?"
    "file mkdir"       "file mkdir ?dir ...?"
    "namespace delete" "namespace delete ?namespace ...?"
    "interp delete"    "interp delete ?path ...?"
    "else"             "if expr1 ?then? body1 elseif expr2 ?then? body2 elseif ... ?else? ?bodyN?"
    "elseif"           "if expr1 ?then? body1 elseif expr2 ?then? body2 elseif ... ?else? ?bodyN?"
    "then"             "if expr1 ?then? body1 elseif expr2 ?then? body2 elseif ... ?else? ?bodyN?"
}

# Option lists for the `UNSAFE`/`ENUMERATE_ONLY` commands (never probed
# with arguments) and for commands that can't be probed without a live
# channel. Copied from the Tcl 8.6 man pages.
array set OPTION_OVERRIDES {
    "puts"           {-nonewline}
    "chan puts"      {-nonewline}
    "unset"          {-nocomplain --}
    "exec"           {-ignorestderr -keepnewline --}
    "source"         {-encoding}
    "load"           {-global -lazy --}
    "unload"         {-nocomplain -keeplibrary --}
    "socket"         {-async -myaddr -myport -server}
    "return"         {-code -errorcode -errorinfo -errorstack -level -options}
    "file copy"      {-force --}
    "file rename"    {-force --}
    "file delete"    {-force --}
    "file link"      {-symbolic -hard}
    "fconfigure"     {-blocking -buffering -buffersize -encoding -eofchar -translation}
    "chan configure" {-blocking -buffering -buffersize -encoding -eofchar -translation}
}

proc root_of {path} { return [lindex $path 0] }
proc may_probe {path} {
    global UNSAFE ENUMERATE_ONLY
    set root [root_of $path]
    if {$root in $UNSAFE} { return 0 }
    if {$root in $ENUMERATE_ONLY && [llength $path] > 1} { return 0 }
    return 1
}

proc probe_args {} {
    set out {}
    for {set i 1} {$i <= 14} {incr i} { lappend out __fenix_probe_$i }
    return $out
}

# The items of a "must be a, b, or c" list -- also "a or b" (no comma
# for two items) and "a, b or c" (no Oxford comma from `clock scan`).
proc split_must_be {list} {
    regsub -all { or } $list "," list
    set items {}
    foreach item [split $list ","] {
        set item [string trim $item]
        if {$item ne ""} { lappend items $item }
    }
    return $items
}

# The "must be ..." list from a subcommand-table error, or {}.
proc subcommands_of {path} {
    if {![may_probe $path]} { return {} }
    # `string is class str` only reaches its "bad class" check once the
    # arity is right, so a probe that fails on arity is retried with one
    # more argument.
    if {[catch {{*}$path __fenix_probe} msg] && [string match "wrong # args:*" $msg]} {
        catch {{*}$path __fenix_probe __fenix_probe} msg
    }
    if {$msg ne ""} {
        if {[regexp {^(?:unknown(?: or ambiguous)? subcommand|bad (?:option|class|argument)) "__fenix_probe": must be (.*)$} $msg -> list]} {
            set names {}
            foreach item [split_must_be $list] {
                # `after`'s list ends in "or an integer" -- not a name;
                # a `-flag` is an option, not a subcommand.
                if {[string match "* *" $item] || [string match "-*" $item]} continue
                lappend names $item
            }
            return $names
        }
    }
    return {}
}

# The `-flag` list from a bad-option error raised *for the probe itself*
# (`array names a __x` complains about a later argument's mode words --
# a list that isn't this command's options), or {}.
proc options_of {path} {
    global OPTION_OVERRIDES
    if {[info exists OPTION_OVERRIDES($path)]} { return $OPTION_OVERRIDES($path) }
    if {![may_probe $path]} { return {} }
    # `clock format 0 -gmt`/`array names a -exact` only parse their
    # options after a positional argument, hence the shapes with a
    # filler in front -- `0` rather than a word so `clock format`'s
    # integer check passes. `after 0 -__probe` would schedule the probe
    # as a script, so `after` only gets the flag-first shapes.
    set shapes {{-__fenix_probe} {-__fenix_probe __a} {-__fenix_probe __a __b} {-__fenix_probe __a __b __c}}
    if {[root_of $path] ne "after"} {
        lappend shapes {__a -__fenix_probe} {__a -__fenix_probe __b} {0 -__fenix_probe} {0 -__fenix_probe __b}
    }
    foreach shape $shapes {
        if {![catch {{*}$path {*}$shape} msg]} continue
        if {[regexp {^bad (?:option|switch|flag) "-__fenix_probe"[,:]? must be (.*)$} $msg -> list]} {
            set names {}
            foreach item [split_must_be $list] {
                if {[string match "-*" $item]} { lappend names $item }
            }
            return $names
        }
    }
    return {}
}

# `clock add` is a Tcl proc, so its arity error names the implementation
# (`::tcl::clock::add clockval ...`) -- the usage line should read the
# way the command is typed.
proc as_typed {path usage} {
    if {[string match "::*" $usage]} {
        regsub {^\S+\s*} $usage "$path " usage
    }
    return $usage
}

proc usage_of {path} {
    return [as_typed $path [raw_usage_of $path]]
}

proc raw_usage_of {path} {
    global OVERRIDES
    if {[info exists OVERRIDES($path)]} { return $OVERRIDES($path) }
    if {[root_of $path] in $::UNSAFE} { return $path }
    if {[catch {{*}$path} msg] && [regexp {^wrong # args: should be "(.*)"$} $msg -> usage]} {
        return $usage
    }
    if {![may_probe $path]} { return $path }
    if {[catch {{*}$path {*}[probe_args]} msg] && [regexp {^wrong # args: should be "(.*)"$} $msg -> usage]} {
        return $usage
    }
    return $path
}

set entries {}
proc walk {path depth} {
    global entries
    set usage [usage_of $path]
    lappend entries [list $path $usage]
    foreach flag [options_of $path] {
        lappend entries [list "$path $flag" $usage]
    }
    if {$depth >= 3} return
    foreach sub [subcommands_of $path] {
        walk "$path $sub" [expr {$depth + 1}]
    }
}

set roots $ROOTS
foreach internal {auto_execok auto_import auto_load auto_load_index auto_qualify tclLog unknown} {
    set roots [lsearch -all -inline -not -exact $roots $internal]
}
foreach root $roots { walk $root 1 }
foreach word $NOT_COMMANDS { lappend entries [list $word $OVERRIDES($word)] }
set entries [lsort -unique -index 0 $entries]

proc rust_str {s} {
    set bs "\\"
    set dq "\""
    return "$dq[string map [list $bs "$bs$bs" $dq "$bs$dq"] $s]$dq"
}

set out [open [lindex $argv 0] w]
fconfigure $out -translation lf
puts $out "//! Generated by `tools/gen_tcl_signatures.tcl` against tclsh [info patchlevel]"
puts $out "//! -- do not edit by hand; rerun the generator instead."
puts $out "//!"
puts $out "//! Every Tcl built-in command path (`string`, `string compare`, `binary"
puts $out "//! encode hex`) paired with the usage line Tcl's own C implementation"
puts $out "//! reports for it. A path ending in a \`-flag\` is one of the previous"
puts $out "//! word's options and carries that command's synopsis. Subcommand lists,"
puts $out "//! option lists and usage strings all come from the interpreter's own"
puts $out "//! argument checking, not from documentation."
puts $out ""
puts $out "/// `(command path, synopsis)`, sorted by path. See `tcl::signature`/"
puts $out "/// `tcl::subcommands` for the lookups built on top of it."
puts $out "pub const SIGNATURES: &\[(&str, &str)\] = &\["
foreach e $entries {
    lassign $e path usage
    puts $out "    ([rust_str $path], [rust_str $usage]),"
}
puts $out "\];"
close $out
puts stderr "wrote [llength $entries] entries"
