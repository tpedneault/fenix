# Editor UI implementation status

Implemented so far on `theme/visual-studio-dark`:

- Breadcrumb display beneath the tabs: project-relative path and enclosing
  multiline scope headers from the syntax tree. Clicking the filename opens
  the document-symbol picker; scope labels jump to their headers.
- `SPC c u` jumps to the nearest enclosing header and `SPC c b` opens the
  document-symbol picker.
- Structural folding is available through `SPC c f`. It collapses the
  smallest scope at the cursor, preserves a visible header/caret, and uses
  a document-to-display row map so scrolling and gutter marks remain aligned.
- The innermost indentation guide at the caret has stronger contrast.
- Floating popups have a thin border and subtle offset shadow.
- Text-buffer status includes language, indentation width, UTF-8 storage,
  and the first line's line-ending convention (LF/CRLF, or no EOL).

Still outstanding from the requested sequence:

- Optional sticky scope headers.
- Completion documentation layout and consistent popup spacing/shortcuts.
- Accurate per-document language-server status, status-bar actions and an
  optional clock.

Breadcrumbs and Tcl folding were rebuilt and verified interactively in Fenix.
The large Rust-buffer latency investigation remains deferred.
