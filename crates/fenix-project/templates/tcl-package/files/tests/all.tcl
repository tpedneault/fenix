package require tcltest 2
namespace import ::tcltest::*
configure -testdir [file dirname [file normalize [info script]]] {*}$argv
runAllTests
