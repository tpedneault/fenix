//! Shells out to external code formatters -- the same "optional tool,
//! degrade to doing nothing rather than a hard error" posture `fenix-
//! docker`/`fenix-git` already established for their own external
//! binaries (`docker`/`podman`, `git`). Currently just Tcl, via `tclfmt`
//! (part of the [tclint](https://github.com/nmoroze/tclint) project,
//! `pip install tclint`) -- more languages get their own function here
//! later, once there's a real second one to add.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// A scratch path under the OS temp dir, unique per call within this
/// process. `tclfmt` only documents taking file paths, not stdin, so
/// formatting goes through a short-lived temp file: write the source,
/// run the tool against it, read stdout back, delete it -- regardless of
/// whether the run succeeded.
fn scratch_path(extension: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("fenix-fmt-{}-{n}.{extension}", std::process::id()))
}

fn run_tclfmt(source: &str, partial: bool) -> Option<String> {
    let path = scratch_path("tcl");
    std::fs::write(&path, source).ok()?;
    let mut cmd = Command::new("tclfmt");
    if partial {
        cmd.arg("--partial");
    }
    cmd.arg(&path);
    let result = cmd.output();
    std::fs::remove_file(&path).ok();
    let output = result.ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Formats a complete Tcl script (`SPC c F`). `None` on any failure --
/// `tclfmt` not on `PATH`, a temp-file I/O error, or `tclfmt` itself
/// rejecting the input (e.g. a syntax error) -- so the caller can just
/// leave the buffer untouched instead of replacing it with nothing.
pub fn format_tcl(source: &str) -> Option<String> {
    run_tclfmt(source, false)
}

/// Formats a fragment of Tcl that isn't necessarily a complete, well-
/// formed script on its own -- a Visual selection, `SPC c f` -- via
/// `tclfmt --partial`, which exists for exactly this (it's what backs
/// range formatting in tclint's own LSP server).
pub fn format_tcl_fragment(source: &str) -> Option<String> {
    run_tclfmt(source, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The tests below all touch the same OS temp directory looking for
    /// "fenix-fmt-*" scratch files -- serialized against each other so
    /// one test's before/after count isn't thrown off by another
    /// creating (and cleaning up) its own scratch file concurrently, the
    /// default test harness running every `#[test]` fn in its own thread.
    static TEMP_DIR_LOCK: Mutex<()> = Mutex::new(());

    fn scratch_file_count() -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with("fenix-fmt-"))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn format_tcl_never_panics_even_without_a_working_tclfmt() {
        let _guard = TEMP_DIR_LOCK.lock().unwrap();
        // Whatever's on this machine (tclfmt missing, or present and
        // happy to reformat this) -- must not panic.
        let _ = format_tcl("proc foo {} {}");
    }

    #[test]
    fn format_tcl_fragment_never_panics_even_without_a_working_tclfmt() {
        let _guard = TEMP_DIR_LOCK.lock().unwrap();
        let _ = format_tcl_fragment("set x 1");
    }

    #[test]
    fn format_tcl_cleans_up_its_scratch_file_regardless_of_outcome() {
        let _guard = TEMP_DIR_LOCK.lock().unwrap();
        let before = scratch_file_count();
        let _ = format_tcl("proc foo {} {}");
        assert_eq!(scratch_file_count(), before);
    }
}
