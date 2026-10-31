# {{name}}

package require Tcl 8.6-

namespace eval {{name_snake}} {
    namespace export greet
}

# Greets WHO -- a stand-in for the package's first real command.
proc {{name_snake}}::greet {{who world}} {
    return "Hello, $who!"
}

package provide {{name_snake}} 0.1.0
