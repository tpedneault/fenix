use std::path::Path;
use std::process::Command;

/// Shells `git args...` with its working directory set to `repo` (every
/// operation here targets a *specific* repository, unlike `fenix-docker`,
/// which talks to one daemon regardless of cwd -- mirrors `fenix-project`'s
/// own `root: &Path`-scoped convention instead), returning stdout split
/// into lines on success, or empty on any failure (missing binary,
/// non-zero exit, not a repo) -- never a hard error.
pub(crate) fn run_lines(repo: &Path, args: &[&str]) -> Vec<String> {
    let Ok(output) = Command::new("git").current_dir(repo).args(args).output() else { return Vec::new() };
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
    match Command::new("git").current_dir(repo).args(args).output() {
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
