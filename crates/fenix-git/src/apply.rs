//! Applying a caller-built patch -- the primitive behind per-hunk
//! staging, unstaging and discarding. The patch itself is built by
//! `fenix-diff` (`hunk_patch`) from a diff this crate produced; this
//! module only pipes text into `git apply` and reports what happened,
//! which is why `fenix-git` still depends on nothing at all.

use std::path::Path;

use crate::process::run_action_stdin;

/// Where a patch lands and which direction it goes.
///
/// The three variants are exactly the three things a hunk key can mean
/// in the Git panel, and each maps to one flag combination `git apply`
/// already understands -- there is no fourth sensible combination
/// (`--reverse` alone against the worktree *is* "discard").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTarget {
    /// `--cached`: add this hunk to the index, leaving the working tree
    /// alone. The patch comes from an unstaged diff (`git diff`).
    Stage,
    /// `--cached --reverse`: take this hunk back out of the index. The
    /// patch comes from a staged diff (`git diff --cached`).
    Unstage,
    /// `--reverse`, no `--cached`: undo this hunk in the working tree,
    /// throwing the change away. Destructive -- callers gate it behind
    /// the panel's own confirmation step, the same way `discard_file`
    /// already is.
    Discard,
}

impl ApplyTarget {
    fn flags(self) -> &'static [&'static str] {
        match self {
            ApplyTarget::Stage => &["--cached"],
            ApplyTarget::Unstage => &["--cached", "--reverse"],
            ApplyTarget::Discard => &["--reverse"],
        }
    }
}

/// Pipes `patch` into `git apply`, targeting the index and/or working
/// tree per `target`. `Ok` means git accepted and applied it; `Err`
/// carries git's own stderr, which is genuinely informative here
/// ("error: patch does not apply", "corrupt patch at line N") and worth
/// surfacing verbatim rather than replacing with a generic message.
///
/// `--whitespace=nowarn` is passed explicitly so behavior doesn't change
/// under a repo or user that has set `apply.whitespace=error` in their
/// git config -- a hunk Fenix generated from git's own diff output is by
/// construction whitespace-identical to what's already in the file, so
/// treating its whitespace as an error would only ever be a false
/// failure on content the user never typed.
pub fn apply_patch(repo: &Path, patch: &str, target: ApplyTarget) -> Result<String, String> {
    let mut args: Vec<String> = vec!["apply".to_string(), "--whitespace=nowarn".to_string()];
    args.extend(target.flags().iter().map(|f| f.to_string()));
    args.push("-".to_string()); // read the patch from stdin
    run_action_stdin(repo, &args, patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::file_diff;
    use crate::test_util::{git, init_repo, TempDir};

    /// A repo with one committed 6-line file, then two separate edits to
    /// it (one near the top, one near the bottom) far enough apart that
    /// `git diff` reports them as two distinct hunks -- the setup every
    /// test here needs to prove one hunk can be staged without the
    /// other coming along.
    fn repo_with_two_hunks(name: &str) -> TempDir {
        let dir = TempDir::new(name);
        init_repo(dir.path());
        dir.write("f.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("f.txt", "one\nTWO\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\nELEVEN\ntwelve\n");
        dir
    }

    /// The whole point: `fenix-diff` parses a real `git diff`, rebuilds
    /// one hunk as a patch, and real `git apply --cached` accepts it.
    /// Unit tests in `fenix-diff` can only prove the text round-trips;
    /// only this can prove git agrees.
    #[test]
    fn staging_one_hunk_stages_exactly_that_hunk() {
        let dir = repo_with_two_hunks("apply_stage_one_hunk");
        let diff = file_diff(dir.path(), "f.txt", false).unwrap();
        let files = fenix_diff::parse(&diff);
        assert_eq!(files[0].hunks.len(), 2, "test setup should produce two hunks, got:\n{diff}");

        let patch = fenix_diff::hunk_patch(&files[0], &files[0].hunks[0]);
        apply_patch(dir.path(), &patch, ApplyTarget::Stage).expect("git should accept the generated patch");

        let staged = file_diff(dir.path(), "f.txt", true).unwrap();
        assert!(staged.contains("+TWO"), "the chosen hunk should be staged:\n{staged}");
        assert!(!staged.contains("ELEVEN"), "the other hunk should not be:\n{staged}");
        // ...and the other one is still sitting unstaged in the worktree.
        let unstaged = file_diff(dir.path(), "f.txt", false).unwrap();
        assert!(unstaged.contains("+ELEVEN"));
        assert!(!unstaged.contains("+TWO"));
    }

    #[test]
    fn unstaging_a_staged_hunk_puts_it_back() {
        let dir = repo_with_two_hunks("apply_unstage_hunk");
        git(dir.path(), &["add", "."]);

        let staged = file_diff(dir.path(), "f.txt", true).unwrap();
        let files = fenix_diff::parse(&staged);
        let patch = fenix_diff::hunk_patch(&files[0], &files[0].hunks[0]);
        apply_patch(dir.path(), &patch, ApplyTarget::Unstage).expect("git should accept the reverse patch");

        let after = file_diff(dir.path(), "f.txt", true).unwrap();
        assert!(!after.contains("TWO"), "the unstaged hunk should be gone from the index:\n{after}");
        assert!(after.contains("ELEVEN"), "the other hunk should still be staged:\n{after}");
    }

    #[test]
    fn discarding_a_hunk_reverts_it_in_the_working_tree() {
        let dir = repo_with_two_hunks("apply_discard_hunk");
        let diff = file_diff(dir.path(), "f.txt", false).unwrap();
        let files = fenix_diff::parse(&diff);
        let patch = fenix_diff::hunk_patch(&files[0], &files[0].hunks[0]);

        apply_patch(dir.path(), &patch, ApplyTarget::Discard).expect("git should accept the reverse patch");

        let contents = std::fs::read_to_string(dir.path().join("f.txt")).unwrap();
        assert!(contents.contains("two"), "the discarded hunk should be back to its committed form");
        assert!(contents.contains("ELEVEN"), "the other edit should be untouched");
    }

    /// The CRLF hazard, end to end against real git rather than just
    /// through the parser: a file whose every line ends `\r\n` has to
    /// stage a hunk without mangling a single line ending.
    #[test]
    fn staging_a_hunk_of_a_crlf_file_keeps_every_line_ending_intact() {
        let dir = TempDir::new("apply_crlf");
        init_repo(dir.path());
        // `core.autocrlf=false` so git stores exactly these bytes and
        // the test is actually about CRLF content, not about git's own
        // conversion.
        git(dir.path(), &["config", "core.autocrlf", "false"]);
        dir.write("c.txt", "one\r\ntwo\r\nthree\r\nfour\r\n");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("c.txt", "one\r\nTWO\r\nthree\r\nfour\r\n");

        let diff = file_diff(dir.path(), "c.txt", false).unwrap();
        let files = fenix_diff::parse(&diff);
        let patch = fenix_diff::hunk_patch(&files[0], &files[0].hunks[0]);
        apply_patch(dir.path(), &patch, ApplyTarget::Stage).expect("git should accept a CRLF patch");

        let staged = file_diff(dir.path(), "c.txt", true).unwrap();
        assert!(staged.contains("+TWO\r"), "the staged line must keep its carriage return:\n{staged:?}");
        // Nothing anywhere in the file quietly became LF-only.
        let worktree = std::fs::read(dir.path().join("c.txt")).unwrap();
        assert_eq!(worktree, b"one\r\nTWO\r\nthree\r\nfour\r\n");
    }

    /// A file with no trailing newline produces a `\ No newline at end
    /// of file` marker that has to survive into the patch, or git
    /// rejects it (or silently adds a newline the user never wanted).
    #[test]
    fn staging_a_hunk_of_a_file_with_no_trailing_newline_works() {
        let dir = TempDir::new("apply_no_trailing_newline");
        init_repo(dir.path());
        dir.write("n.txt", "alpha\nbeta");
        git(dir.path(), &["add", "."]);
        git(dir.path(), &["commit", "-q", "-m", "initial"]);
        dir.write("n.txt", "alpha\nBETA");

        let diff = file_diff(dir.path(), "n.txt", false).unwrap();
        assert!(diff.contains("\\ No newline at end of file"), "test setup should produce the marker:\n{diff}");
        let files = fenix_diff::parse(&diff);
        let patch = fenix_diff::hunk_patch(&files[0], &files[0].hunks[0]);
        apply_patch(dir.path(), &patch, ApplyTarget::Stage).expect("git should accept a no-trailing-newline patch");

        let staged = file_diff(dir.path(), "n.txt", true).unwrap();
        assert!(staged.contains("+BETA"));
        // The file itself still has no trailing newline.
        assert_eq!(std::fs::read(dir.path().join("n.txt")).unwrap(), b"alpha\nBETA");
    }

    #[test]
    fn a_patch_that_does_not_apply_reports_gits_own_error() {
        let dir = repo_with_two_hunks("apply_rejects");
        let bogus = "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -1,1 +1,1 @@\n-nothing like the real content\n+replacement\n";
        let err = apply_patch(dir.path(), bogus, ApplyTarget::Stage).unwrap_err();
        assert!(err.contains("apply") || err.contains("patch"), "expected git's own apply error, got: {err}");
    }
}
