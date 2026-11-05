//! What start-up asks before there's an `App`: whether this is a fresh
//! install's first run, and whether an extension opens the file named on
//! the command line itself.

use std::path::Path;

/// What happened before the window was up, to be said once it is.
static SAY_LATER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// `what`, said in the window once it's up (`said_later`).
pub(crate) fn say_later(what: String) {
    SAY_LATER.lock().unwrap_or_else(|e| e.into_inner()).push(what);
}

/// What `say_later` was given, taken.
pub(crate) fn said_later() -> Vec<String> {
    std::mem::take(&mut *SAY_LATER.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Whether this is a fresh install's first run: no `settings.toml` yet.
/// It's welcomed.
pub(crate) fn first_run() -> bool {
    static FIRST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FIRST.get_or_init(|| {
        if cfg!(test) || crate::installed::safe_mode() {
            return false;
        }
        let Some(settings) = fenix_storage::paths::Roots::current().map(|roots| roots.settings_file()) else { return false };
        !settings.exists()
    })
}

/// Whether an extension opens `path` itself (`contributes.opens`).
pub(crate) fn claims_file(path: &Path) -> bool {
    crate::installed::code_extensions().iter().any(|e| fenix_extensions::manifest::file_matches(&e.manifest.contributes.opens, path, None))
}
