# Deferred: large Rust buffer input latency

Status: deferred at the user's request; root cause unconfirmed.

Reproduction: edit `crates/fenix-gui/src/app.rs` on
`theme/visual-studio-dark`. Normal-mode `d e` reportedly takes 300–500 ms;
holding `j` visibly jumps across lines. The user's Tcl test environment is
responsive. Investigate file size and Rust syntax processing as variables,
without assuming the theme is the cause.

Earlier tab-history and indentation-guide optimizations did not resolve the
reported delay. Their unit tests verify behavior, not end-to-end latency.
A synthetic tab shaping benchmark measured less than 1 ms per iteration in
the debug test build; it does not reproduce a full editor frame.

Next investigation:

- Record executable/build profile, buffer size, viewport, LSP state and theme.
- Measure key dispatch, Vim edit, clipboard, syntax update/highlighting, LSP
  synchronization, text shaping, GPU acquisition/presentation and disk polling.
- Compare this same file with syntax disabled, and with the previous theme;
  compare Rust and plain-text buffers of matching sizes.
- Establish repeatable baseline and post-fix timings for `d e` and held `j`.
- Target sub-16-ms routine input/frame work on the reference machine; report
  median, p95 and worst-case latency, with no lost key events.

Uncommitted opt-in profiling scaffolding remains in the checkout from the
interrupted investigation (`FENIX_PROFILE=1`). Validate and extend it before
using its output as evidence; no interactive latency fix is confirmed.
