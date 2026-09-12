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
#     which is the exact usage line the C implementation carries.
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
            foreach item [split $list ","] {
                set item [string trim $item]
                regsub {^or } $item {} item
                # `after`'s list ends in "or an integer" -- not a name;
                # a `-flag` is an option, not a subcommand.
                if {$item eq "" || [string match "* *" $item] || [string match "-*" $item]} continue
                lappend names $item
            }
            return $names
        }
    }
    return {}
}

proc usage_of {path} {
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
    lappend entries [list $path [usage_of $path]]
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
puts $out "//! reports for it. Subcommand lists and usage strings both come from"
puts $out "//! the interpreter's argument checking, not from documentation."
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
