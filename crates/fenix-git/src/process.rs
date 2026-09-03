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
    // Fenix has no terminal for git to hand an interactive prompt to, so
    // anything that would open one has to be defused up front or the
    // command hangs forever with no way to answer it and no sign of why:
    //
    // - `GIT_EDITOR`/`GIT_SEQUENCE_EDITOR`: `rebase --continue`, `merge`,
    //   `revert` and `cherry-pick` all launch an editor for a commit or
    //   todo message. `true` exits 0 immediately, which git reads as
    //   "accept the prepared message unchanged" -- exactly what an
    //   editor-less client wants.
    // - `GIT_TERMINAL_PROMPT=0`: makes `fetch`/`push` against a repo
    //   needing credentials fail with a real error instead of blocking
    //   on a username prompt nobody can see.
    cmd.env("GIT_EDITOR", "true").env("GIT_SEQUENCE_EDITOR", "true").env("GIT_TERMINAL_PROMPT", "0");
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

/// `run_action`, but with `stdin` piped into the child -- what `git
/// apply` needs, since a patch arrives as text this process built in
/// memory (see `apply::apply_patch`) rather than as a file on disk.
/// Same never-panics, `Ok(stdout)`/`Err(stderr)` contract as
/// `run_action`; a write failure on the pipe is reported the same way a
/// non-zero exit is, since from the caller's point of view both mean
/// "the patch didn't apply."
pub(crate) fn run_action_stdin(repo: &Path, args: &[String], stdin: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::Stdio;

    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut cmd = git_command(repo, &args);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return Err(format!("couldn't run git: {err}")),
    };
    // Dropped (closing the pipe) before `wait_with_output`, or git would
    // block forever waiting for an EOF that never comes.
    if let Some(mut pipe) = child.stdin.take() {
        if let Err(err) = pipe.write_all(stdin.as_bytes()) {
            return Err(format!("couldn't send the patch to git: {err}"));
        }
    }
    match child.wait_with_output() {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).into_owned()),
        Err(err) => Err(format!("couldn't run git: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_action_stdin_reports_an_error_for_an_unrecognized_subcommand() {
        let result = run_action_stdin(&std::env::temp_dir(), &["not-a-real-subcommand".to_string()], "payload\n");
        assert!(result.is_err());
    }

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
