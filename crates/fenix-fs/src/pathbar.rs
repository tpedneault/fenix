//! Turning what somebody typed into somewhere they can go, and
//! completing it while they type.
//!
//! The explorer had no way to enter a path at all: you walked there one
//! `Enter` at a time from wherever you started, which for
//! `\\nas\media\projects\2026` is not a thing anyone does twice. This is
//! the half that reads what was typed; the prompt around it lives in
//! the host.

use std::path::{Path, PathBuf};

/// Turns typed text into a real path.
///
/// Handles the four things people actually type and no more: `~` for
/// home, `%VAR%` for an environment variable, surrounding quotes
/// (because a path copied out of Explorer arrives wrapped in them), and
/// forward slashes on Windows (because everyone types them and Windows
/// accepts them).
pub fn expand(input: &str) -> PathBuf {
    let trimmed = input.trim().trim_matches('"');
    let with_vars = expand_vars(trimmed);
    let expanded = expand_home(&with_vars);
    PathBuf::from(expanded)
}

/// `~`, or `~/rest`, or `~\rest` -- but not `~something`, which is a
/// perfectly ordinary filename and not a home directory reference.
fn expand_home(input: &str) -> String {
    let Some(rest) = input.strip_prefix('~') else { return input.to_string() };
    if !(rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\')) {
        return input.to_string();
    }
    let Some(home) = home_dir() else { return input.to_string() };
    format!("{}{}", home.display(), rest)
}

/// `%APPDATA%` and friends. An unset variable is left exactly as typed
/// rather than replaced with emptiness -- silently turning
/// `%TYPO%\notes` into `\notes` would send you to the root of the
/// current drive, which is both wrong and confusing.
fn expand_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(open) = rest.find('%') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('%') else { break };
        let name = &after[..close];
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => {
                out.push_str(&rest[..open]);
                out.push_str(&value);
            }
            _ => out.push_str(&rest[..open + 1 + close + 1]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// How a partially-typed path splits for completion: everything that is
/// already a directory, and the fragment being typed inside it.
fn split_for_completion(input: &str) -> (PathBuf, String) {
    let expanded = expand(input);
    let text = expanded.to_string_lossy().into_owned();
    // A trailing separator means "inside this directory", with nothing
    // typed yet -- so everything in it is a candidate.
    if text.ends_with('\\') || text.ends_with('/') {
        return (PathBuf::from(text), String::new());
    }
    match text.rfind(['\\', '/']) {
        Some(at) => {
            let (dir, name) = text.split_at(at);
            // `C:\foo` splits to `C:` + `\foo`; the parent of a
            // top-level entry is the drive *root*, not the drive.
            let dir = if dir.is_empty() || dir.ends_with(':') { format!("{dir}\\") } else { dir.to_string() };
            (PathBuf::from(dir), name[1..].to_string())
        }
        None => (PathBuf::from("."), text),
    }
}

/// Directories under `input`'s parent whose names continue what has
/// been typed, as full path strings ready to replace the input.
///
/// Directories only. A path bar exists to get you somewhere, and
/// offering files would fill the list with things that are not
/// somewhere -- opening a file has its own key.
///
/// Case-insensitive on Windows, because that is what its filesystem is
/// and typing `c:\us` should find `C:\Users`.
pub fn complete(input: &str) -> Vec<String> {
    let (dir, partial) = split_for_completion(input);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let needle = fold_case(&partial);
    let mut out: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| fold_case(name).starts_with(&needle))
        .map(|name| join_for_display(&dir, &name))
        .collect();
    out.sort_by_key(|path| fold_case(path));
    out
}

/// The longest text every candidate agrees on -- what Tab should fill
/// in when there is more than one match, so repeated Tabs make
/// progress instead of doing nothing.
pub fn common_prefix(candidates: &[String]) -> Option<String> {
    let first = candidates.first()?;
    let mut len = first.chars().count();
    for candidate in &candidates[1..] {
        len = first.chars().zip(candidate.chars()).take(len).take_while(|(a, b)| fold_char(*a) == fold_char(*b)).count();
    }
    Some(first.chars().take(len).collect())
}

/// Appends `name` to `dir` as text, keeping a single separator. Not
/// `Path::join`, which would give `\\nas\media` a `\\` it did not have
/// and turn a UNC root into something that no longer parses.
fn join_for_display(dir: &Path, name: &str) -> String {
    let dir = dir.to_string_lossy();
    if dir.ends_with('\\') || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

fn fold_case(s: &str) -> String {
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

fn fold_char(c: char) -> char {
    if cfg!(windows) {
        c.to_ascii_lowercase()
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn a_plain_path_is_left_alone() {
        assert_eq!(expand(r"C:\Users\thoma"), PathBuf::from(r"C:\Users\thoma"));
    }

    #[test]
    fn a_path_copied_out_of_explorer_arrives_wrapped_in_quotes() {
        assert_eq!(expand("\"C:\\Program Files\""), PathBuf::from(r"C:\Program Files"));
    }

    #[test]
    fn surrounding_whitespace_is_not_part_of_the_path() {
        assert_eq!(expand("  C:\\Users  "), PathBuf::from(r"C:\Users"));
    }

    #[test]
    fn tilde_means_home_only_when_it_stands_alone_or_starts_a_path() {
        let home = home_dir().expect("a home directory");
        assert_eq!(expand("~"), home);
        assert_eq!(expand("~/Downloads"), PathBuf::from(format!("{}/Downloads", home.display())));
        assert_eq!(expand(r"~\Downloads"), PathBuf::from(format!("{}\\Downloads", home.display())));
        // `~notes.txt` is a filename, not somebody's home directory.
        assert_eq!(expand("~notes.txt"), PathBuf::from("~notes.txt"));
    }

    #[test]
    fn an_environment_variable_is_replaced_by_its_value() {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).expect("a home variable");
        let expanded = if cfg!(windows) { expand(r"%USERPROFILE%\Documents") } else { expand("%HOME%/Documents") };
        assert!(expanded.to_string_lossy().starts_with(&home), "got {expanded:?}");
        assert!(expanded.to_string_lossy().ends_with("Documents"));
    }

    #[test]
    fn an_unset_variable_is_left_as_typed_rather_than_erased() {
        // Erasing it would turn `%TYPO%\notes` into `\notes`, which is a
        // real place, just not the one that was meant.
        assert_eq!(expand(r"%NO_SUCH_VARIABLE_HERE%\notes"), PathBuf::from(r"%NO_SUCH_VARIABLE_HERE%\notes"));
    }

    #[test]
    fn a_lone_percent_sign_is_not_treated_as_a_variable() {
        assert_eq!(expand(r"C:\100% done"), PathBuf::from(r"C:\100% done"));
    }

    #[test]
    fn a_unc_path_survives_expansion_unchanged() {
        // The one shape most likely to be mangled by path handling, and
        // the whole reason this feature exists.
        assert_eq!(expand(r"\\nas\media\projects"), PathBuf::from(r"\\nas\media\projects"));
    }

    // -- Completion ------------------------------------------------------

    fn dir_with_children(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        dir.mkdir("alpha");
        dir.mkdir("alpine");
        dir.mkdir("beta");
        dir.touch("alpha.txt");
        dir
    }

    #[test]
    fn completion_offers_the_directories_that_continue_what_was_typed() {
        let dir = dir_with_children("complete_prefix");
        let typed = format!("{}\\alp", dir.path().display());

        let candidates = complete(&typed);

        let names: Vec<String> = candidates.iter().map(|c| c.rsplit('\\').next().unwrap().to_string()).collect();
        assert_eq!(names, vec!["alpha", "alpine"]);
    }

    #[test]
    fn completion_offers_directories_only() {
        // A path bar gets you somewhere; a file is not somewhere.
        let dir = dir_with_children("complete_dirs_only");
        let candidates = complete(&format!("{}\\alpha", dir.path().display()));
        assert!(candidates.iter().all(|c| !c.ends_with(".txt")), "got {candidates:?}");
    }

    #[test]
    fn a_trailing_separator_offers_everything_inside() {
        let dir = dir_with_children("complete_trailing_sep");
        let candidates = complete(&format!("{}\\", dir.path().display()));
        assert_eq!(candidates.len(), 3, "got {candidates:?}");
    }

    #[test]
    fn completion_returns_full_paths_ready_to_replace_the_input() {
        // So accepting one is an assignment, not a splice the caller has
        // to work out how to perform.
        let dir = dir_with_children("complete_full_paths");
        let candidates = complete(&format!("{}\\bet", dir.path().display()));
        assert_eq!(candidates, vec![format!("{}\\beta", dir.path().display())]);
    }

    #[cfg(windows)]
    #[test]
    fn completion_ignores_case_because_the_filesystem_does() {
        let dir = dir_with_children("complete_case");
        let candidates = complete(&format!("{}\\ALP", dir.path().display()));
        assert_eq!(candidates.len(), 2, "got {candidates:?}");
    }

    #[test]
    fn completing_somewhere_that_does_not_exist_offers_nothing() {
        assert!(complete(r"C:\no-such-directory-anywhere\x").is_empty());
    }

    #[test]
    fn the_common_prefix_is_what_tab_can_safely_fill_in() {
        let candidates = vec![r"C:\dir\alpha".to_string(), r"C:\dir\alpine".to_string()];
        assert_eq!(common_prefix(&candidates).as_deref(), Some(r"C:\dir\alp"));
    }

    #[test]
    fn one_candidate_completes_to_itself() {
        let candidates = vec![r"C:\dir\beta".to_string()];
        assert_eq!(common_prefix(&candidates).as_deref(), Some(r"C:\dir\beta"));
    }

    #[test]
    fn no_candidates_have_no_common_prefix() {
        assert_eq!(common_prefix(&[]), None);
    }

    #[test]
    fn candidates_sharing_nothing_have_an_empty_common_prefix() {
        let candidates = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(common_prefix(&candidates).as_deref(), Some(""));
    }
}
