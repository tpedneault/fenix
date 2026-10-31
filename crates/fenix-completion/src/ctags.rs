use std::path::PathBuf;

/// One named place in a file to jump to: what the symbol pickers list
/// (`SPC c o`'s document scopes). The name ctags would give a definition
/// -- Tcl's own scan with ctags is the Tcl extension's now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    /// What the picker shows and filters by.
    pub name: String,
    pub file: PathBuf,
    /// From 1.
    pub line: usize,
}
