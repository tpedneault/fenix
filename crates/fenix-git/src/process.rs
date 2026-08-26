use std::path::Path;
use std::process::Command;

/// `git` on Windows, run with this flag, doesn't flash a console window
/// per spawn -- `fenix-gui` has no console of its own to inherit, same
/// reasoning as `fenix-completion::ctags::run`'s own use of this flag.
/// Purely cosmetic (no bearing on why a run fails), but real spam
/// reduction given how often this crate's callers shell out.
fn git_command(repo: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Shells `git args...` with its working directory set to `repo` (every
/// operation here targets a *specific* repository, unlike `fenix-docker`,
/// which talks to one daemon regardless of cwd -- mirrors `fenix-project`'s
/// own `root: &Path`-scoped convention instead), returning stdout split
/// into lines on success, or empty on any failure (missing binary,
/// non-zero exit, not a repo) -- never a hard error.
pub(crate) fn run_lines(repo: &Path, args: &[&str]) -> Vec<String> {
    let Ok(output) = git_command(repo, args).output() else { return Vec::new() };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout).lines().map(str::to_string).collect()
}

/// Runs `git args...` in `repo`, returning stdout on success or stderr
/// (or a synthetic message if the binary itself couldn't be launched) on
/// failure -- the caller decides what to do with either (render into a
/// buffer, show as an error, etc.). Same shape as `fenix-docker::process::
/// run_action`.
pub(crate) fn run_action(repo: &Path, args: &[String]) -> Result<String, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match git_command(repo, &args).output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
        Err(err) => Err(format!("couldn't run git: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_lines_returns_empty_outside_a_git_repo() {
        // The OS temp dir itself is never a git repo.
        assert!(run_lines(&std::env::temp_dir(), &["status"]).is_empty());
    }

    #[test]
    fn run_action_reports_an_error_for_an_unrecognized_subcommand() {
        let result = run_action(&std::env::temp_dir(), &["not-a-real-subcommand".to_string()]);
        assert!(result.is_err());
    }
}
