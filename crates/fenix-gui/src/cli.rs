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
//! that starts with `-` is never taken for a file: an option Fenix
//! doesn't know is warned of (`unknown_flags`) and otherwise ignored;
//! `--` ends the flags, for a file that starts with a dash. `--help` and
//! `--version` print and exit (`info_request`).

use std::path::{Path, PathBuf};

use fenix_tasks::{parse_path_position, PathPosition};

pub const NEW_WINDOW_FLAG: &str = "--new-window";
pub const GOTO_FLAG: &str = "--goto";
pub const PROJECT_FLAG: &str = "--project";

/// What `fenix --help` prints.
pub fn help_text() -> String {
    format!(
        "Fenix {version}, a keyboard-first editor

Usage:
  fenix [options] [file[:line[:col]]...]

Opening:
  file[:line[:col]]          Open a file, with the cursor at a line and column
  --goto <file:line[:col]>   Always read a position, even when a file has that name
  --project <dir>            Switch to a project, as SPC p p does
  --new-window               Open in another window of the running Fenix
  --                         End the options: what follows is a file

Notifying:
  --notify <summary>         Show a notification in the running Fenix, with:
    --body <text>            what it says under its summary
    --urgency <level>        low, normal (the default) or critical
    --tag <tag>              replacing the one with the same tag (a progress)
    --progress <0-100>       a bar along its foot

Running:
  --disable-extensions       Start without extensions (safe mode), in a window of its own
  --console                  Show what Fenix prints (Windows)
  --mcp                      Be an MCP server on stdio for a coding agent, talking to the running Fenix

Extensions:
  --install-extension <file|folder|id>   Install one (an id comes from the registry)
  --uninstall-extension <id>             Remove one
  --list-extensions                      List what's installed
  --machine                              With those: the machine's extensions, for every user

  --version, -V              Print the version
  --help, -h                 Print this

A running Fenix gets what a later `fenix` asks for. docs/CONTROL.md has the rest.
",
        version = crate::installed::describe()
    )
}

/// What `fenix --version` prints: `fenix 1.5.2`, or for a nightly
/// `fenix 1.6.0-nightly.20261112 (a1b2c3d4e5f6)`, its commit.
pub fn version_text() -> String {
    format!("fenix {}", crate::installed::describe())
}

/// `--help` or `--version`, when the command line asks for either before
/// any `--`: printed, and Fenix exits.
pub fn info_request(args: &[String]) -> Option<String> {
    for arg in args.iter().take_while(|a| *a != "--") {
        match arg.as_str() {
            "--help" | "-h" | "-?" => return Some(help_text()),
            "--version" | "-V" => return Some(version_text()),
            _ => {}
        }
    }
    None
}

/// Every option Fenix knows, those another part reads included.
const KNOWN_FLAGS: &[&str] = &[
    NEW_WINDOW_FLAG,
    GOTO_FLAG,
    PROJECT_FLAG,
    "--console",
    "--mcp",
    "--disable-extensions",
    "--install-extension",
    "--uninstall-extension",
    "--list-extensions",
    "--machine",
    "--help",
    "--version",
    "--notify",
    "--body",
    "--urgency",
    "--tag",
    "--progress",
];

/// The options in `args` Fenix doesn't know, as typed: `--projet` is
/// warned of, not quietly dropped. A value after an option that takes
/// one isn't an option.
pub fn unknown_flags(args: &[String]) -> Vec<String> {
    let takes_value = [GOTO_FLAG, PROJECT_FLAG, "--install-extension", "--uninstall-extension", "--notify", "--body", "--urgency", "--tag", "--progress"];
    let mut unknown = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            continue;
        }
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if takes_value.contains(&flag) && !arg.contains('=') {
            args.next();
        }
        if !KNOWN_FLAGS.contains(&flag) && !matches!(flag, "-h" | "-V" | "-?") {
            unknown.push(arg.clone());
        }
    }
    unknown
}

/// `fenix --notify <summary> [--body ..] [--urgency ..] [--tag ..]
/// [--progress ..]`: the notification to hand the running Fenix, or why
/// it doesn't read. None when there's no `--notify`.
pub fn notify_request(args: &[String]) -> Option<Result<fenix_control::NotifyParams, String>> {
    let mut summary = None;
    let mut params = fenix_control::NotifyParams { summary: String::new(), body: String::new(), urgency: None, source: Some("cli".into()), tag: None, progress: None, actions: Vec::new() };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next().cloned());
        match flag {
            "--notify" => summary = Some(value().unwrap_or_default()),
            "--body" => params.body = value().unwrap_or_default(),
            "--urgency" => params.urgency = value(),
            "--tag" => params.tag = value(),
            "--progress" => match value().map(|v| v.trim_end_matches('%').parse::<u8>()) {
                Some(Ok(p)) if p <= 100 => params.progress = Some(p),
                _ => return Some(Err("--progress takes 0 to 100".into())),
            },
            _ => {}
        }
    }
    let summary = summary?;
    if summary.trim().is_empty() {
        return Some(Err("--notify needs a summary: fenix --notify \"Build done\"".into()));
    }
    if params.urgency.as_deref().is_some_and(|u| !matches!(u, "low" | "normal" | "critical")) {
        return Some(Err("--urgency is low, normal or critical".into()));
    }
    params.summary = summary;
    Some(Ok(params))
}

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

/// A folder named as a file (`fenix C:\code\orbit`) is a project to
/// open, as `--project` is, rather than a file Windows won't read: the
/// first such folder becomes the project when none was given, and the
/// rest are dropped. `is_dir` says which paths are folders.
pub fn folders_as_project(mut invocation: Invocation, is_dir: impl Fn(&Path) -> bool) -> Invocation {
    let (folders, files): (Vec<PathPosition>, Vec<PathPosition>) = std::mem::take(&mut invocation.files).into_iter().partition(|f| f.line.is_none() && is_dir(&f.path));
    invocation.files = files;
    if invocation.project.is_none() {
        invocation.project = folders.into_iter().next().map(|f| f.path);
    }
    invocation
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

    #[test]
    fn help_and_version_are_printed_not_handed_on() {
        let help = info_request(&args(&["--help"])).unwrap();
        for flag in ["--mcp", "--install-extension", "--list-extensions", "--disable-extensions", "--goto", "--project", "--new-window", "--version"] {
            assert!(help.contains(flag), "{flag} missing from:\n{help}");
        }
        assert_eq!(info_request(&args(&["-V"])).unwrap(), format!("fenix {}", crate::installed::describe()));
        assert_eq!(info_request(&args(&["a.rs", "--version"])), Some(version_text()));
        assert_eq!(info_request(&args(&["--", "--help"])), None, "after -- it's a file");
        assert_eq!(info_request(&args(&["a.rs"])), None);
    }

    #[test]
    fn an_option_fenix_doesnt_know_is_named() {
        assert_eq!(unknown_flags(&args(&["--projet", "repo", "a.rs"])), ["--projet"]);
        assert!(unknown_flags(&args(&["--project", "--odd-folder", "--goto=x:1", "--console", "--mcp", "--", "--file"])).is_empty());
        assert!(unknown_flags(&args(&["--install-extension", "x.fenixext", "--machine"])).is_empty());
    }

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

    /// `fenix <folder>` opens it as the project, not as a file.
    #[test]
    fn a_folder_given_as_a_file_is_the_project() {
        let inv = parse(&args(&["orbit", "notes.md"]), &cwd(), |_| false).unwrap();
        let inv = folders_as_project(inv, |p| p.ends_with("orbit"));
        assert_eq!(inv.project, Some(cwd().join("orbit")));
        assert_eq!(inv.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), vec![cwd().join("notes.md")]);
        let inv = parse(&args(&["--project", "a", "b"]), &cwd(), |_| false).unwrap();
        let inv = folders_as_project(inv, |p| p.ends_with("b"));
        assert_eq!(inv.project, Some(cwd().join("a")), "--project still wins");
        assert!(inv.files.is_empty());
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

    #[test]
    fn notify_reads_its_options_and_says_whats_wrong() {
        assert_eq!(notify_request(&args(&["a.rs"])), None);
        let p = notify_request(&args(&["--notify", "Build done", "--body=in 41 s", "--urgency", "low", "--progress", "40%", "--tag", "b"])).unwrap().unwrap();
        assert_eq!((p.summary.as_str(), p.body.as_str(), p.urgency.as_deref(), p.progress, p.tag.as_deref(), p.source.as_deref()), ("Build done", "in 41 s", Some("low"), Some(40), Some("b"), Some("cli")));
        assert!(notify_request(&args(&["--notify", " "])).unwrap().is_err());
        assert!(notify_request(&args(&["--notify", "x", "--urgency", "loud"])).unwrap().is_err());
        assert!(notify_request(&args(&["--notify", "x", "--progress", "140"])).unwrap().is_err());
        assert!(unknown_flags(&args(&["--notify", "x", "--body", "--y"])).is_empty(), "a value isn't an option");
    }

}
