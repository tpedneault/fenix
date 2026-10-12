//! What a `fenix` command line asks for, whether this launch becomes
//! the editor or hands it all to the one already running (`ipc`).
//!
//! ```text
//! fenix [--new-window] [--console] [--project <dir>] [--goto <file:line[:col]>] [file[:line[:col]]...]
//! ```
//!
//! A file argument may carry a position, `src/main.rs:12:3`, unless a
//! file of exactly that name exists (a Linux file may end in `:12`).
//! `--goto` always reads one, and so suits a tool that builds the
//! argument (a `git difftool`, a terminal's link handler). Anything else
//! that starts with `-` is ignored, never taken for a file; `--` ends
//! the flags, for a file that starts with a dash.

use std::path::{Path, PathBuf};

use fenix_tasks::{parse_path_position, PathPosition};

pub const NEW_WINDOW_FLAG: &str = "--new-window";
pub const GOTO_FLAG: &str = "--goto";
pub const PROJECT_FLAG: &str = "--project";

/// One parsed command line.
#[derive(Debug, Default, PartialEq)]
pub struct Invocation {
    /// Open another window rather than using the focused one.
    pub new_window: bool,
    /// Files to open, in order; the last one ends up focused.
    pub files: Vec<PathPosition>,
    /// A project to switch to, as `SPC p p` does.
    pub project: Option<PathBuf>,
}

/// Parses `args` (without the program name). `exists` says whether a
/// path is there as typed, so a file named `notes:12` opens as itself.
/// Relative paths are made absolute against `cwd`: a running Fenix has
/// its own working directory, and a relative path handed to it would
/// open the wrong file.
pub fn parse(
    args: &[String],
    cwd: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Result<Invocation, String> {
    let mut invocation = Invocation::default();
    let mut args = args.iter();
    let mut flags_done = false;
    while let Some(arg) = args.next() {
        if flags_done || !arg.starts_with('-') {
            let position = if exists(&cwd.join(arg)) {
                PathPosition {
                    path: PathBuf::from(arg),
                    line: None,
                    column: None,
                }
            } else {
                parse_path_position(arg)
            };
            invocation.files.push(position);
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        let mut value = |what: &str| {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| format!("{flag} needs {what}"))
        };
        match flag {
            "--" => flags_done = true,
            NEW_WINDOW_FLAG => invocation.new_window = true,
            GOTO_FLAG => invocation
                .files
                .push(parse_path_position(&value("a file:line[:column]")?)),
            PROJECT_FLAG => invocation.project = Some(PathBuf::from(value("a folder")?)),
            _ => {}
        }
    }
    for file in &mut invocation.files {
        file.path = absolute(cwd, &file.path);
    }
    if let Some(project) = &mut invocation.project {
        *project = absolute(cwd, project);
    }
    Ok(invocation)
}

/// `path` against `cwd`, without touching the disk: a file that doesn't
/// exist yet is still a file to create.
fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn cwd() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\work")
        } else {
            PathBuf::from("/work")
        }
    }

    fn parse_none_exist(list: &[&str]) -> Invocation {
        parse(&args(list), &cwd(), |_| false).unwrap()
    }

    fn at(path: PathBuf, line: Option<usize>, column: Option<usize>) -> PathPosition {
        PathPosition { path, line, column }
    }

    #[test]
    fn plain_files_keep_their_order_and_become_absolute() {
        let inv = parse_none_exist(&["a.rs", "src/b.rs"]);
        assert_eq!(
            inv.files,
            vec![
                at(cwd().join("a.rs"), None, None),
                at(cwd().join("src/b.rs"), None, None)
            ]
        );
        assert!(!inv.new_window);
        assert_eq!(inv.project, None);
    }

    #[test]
    fn a_file_argument_may_carry_a_position() {
        let inv = parse_none_exist(&["a.rs:12", "b.rs:3:7"]);
        assert_eq!(
            inv.files,
            vec![
                at(cwd().join("a.rs"), Some(12), None),
                at(cwd().join("b.rs"), Some(3), Some(7))
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_drive_letter_is_not_a_position() {
        let inv = parse_none_exist(&[r"C:\x.rs:12:3", r"D:\y.rs"]);
        assert_eq!(
            inv.files,
            vec![
                at(PathBuf::from(r"C:\x.rs"), Some(12), Some(3)),
                at(PathBuf::from(r"D:\y.rs"), None, None)
            ]
        );
    }

    #[test]
    fn a_file_that_exists_under_its_full_name_opens_as_itself() {
        let inv = parse(&args(&["notes:12"]), &cwd(), |p| p.ends_with("notes:12")).unwrap();
        assert_eq!(inv.files, vec![at(cwd().join("notes:12"), None, None)]);
    }

    #[test]
    fn goto_always_reads_a_position() {
        let inv = parse(
            &args(&["--goto", "notes:12", "--goto=x.rs:1:2"]),
            &cwd(),
            |_| true,
        )
        .unwrap();
        assert_eq!(
            inv.files,
            vec![
                at(cwd().join("notes"), Some(12), None),
                at(cwd().join("x.rs"), Some(1), Some(2))
            ]
        );
    }

    #[test]
    fn project_takes_a_folder() {
        let inv = parse_none_exist(&["--project", "repo", "a.rs"]);
        assert_eq!(inv.project, Some(cwd().join("repo")));
        assert_eq!(inv.files, vec![at(cwd().join("a.rs"), None, None)]);
        assert_eq!(
            parse_none_exist(&["--project=repo"]).project,
            Some(cwd().join("repo"))
        );
    }

    #[test]
    fn a_flag_missing_its_value_is_an_error() {
        assert!(parse(&args(&["--goto"]), &cwd(), |_| false)
            .unwrap_err()
            .contains("--goto"));
        assert!(parse(&args(&["--project"]), &cwd(), |_| false)
            .unwrap_err()
            .contains("--project"));
    }

    #[test]
    fn unknown_flags_are_dropped_and_double_dash_ends_the_flags() {
        let inv = parse_none_exist(&["--console", "--colour", "--new-window", "--", "-weird.txt"]);
        assert!(inv.new_window);
        assert_eq!(inv.files, vec![at(cwd().join("-weird.txt"), None, None)]);
    }
}
