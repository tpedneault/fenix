//! The files that have been moved over to `fenix-vfs` stay moved.
//!
//! Code that touches a project's files has to take a remote path as
//! readily as a local one, which `std::fs` doesn't: given a remote path
//! it finds nothing. Each file listed here was gone through and now asks
//! `fenix-vfs`; a new `std::fs` call in one would work on this machine's
//! files and quietly fail on a remote project's. Add a file here when
//! it's been moved over.

use std::path::Path;

/// Files whose code (above their tests) makes no `std::fs` call.
const MOVED: &[&str] = &["crates/fenix-explorer/src/state.rs"];

/// Files where it's allowed in the lines named: the local half of
/// something that has a remote half beside it.
const SPLIT: &[(&str, &[&str])] = &[("crates/fenix-project/src/files.rs", &["std::fs::read_dir(dir)"])];

fn code(path: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join(path)).unwrap_or_else(|error| panic!("{path}: {error}"));
    // The tests below make files to test with, as they should.
    text.split("#[cfg(test)]").next().unwrap_or_default().to_string()
}

fn calls(code: &str) -> Vec<(usize, String)> {
    code.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| line.contains("std::fs::") || line.contains(" fs::") || line.contains("(fs::") || line.contains("File::open") || line.contains("File::create"))
        .map(|(number, line)| (number + 1, line.trim().to_string()))
        .collect()
}

#[test]
fn files_moved_to_the_vfs_make_no_std_fs_calls() {
    for path in MOVED {
        let found = calls(&code(path));
        assert!(found.is_empty(), "{path} touches this machine's disk directly, which a remote project's files aren't on:\n{found:#?}\nUse fenix_vfs (read, write, metadata, list, rename, remove, create_dir_all) instead.");
    }
    for (path, allowed) in SPLIT {
        let found: Vec<_> = calls(&code(path)).into_iter().filter(|(_, line)| !allowed.iter().any(|ok| line.contains(ok))).collect();
        assert!(found.is_empty(), "{path} has a new call on this machine's disk:\n{found:#?}\nGive it a remote half (fenix_vfs), or add it to SPLIT if it has one.");
    }
}
