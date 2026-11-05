use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// A real, uniquely-named directory under the OS temp dir, removed on
/// drop -- mirrors `fenix-explorer`'s own `test_util::TempDir`. Every
/// `fenix-git` function shells a real `git` binary against a real
/// repository, so its tests do the same (same discipline `fenix-project`'s
/// `grep_project`/`list_project_files` tests already established for
/// shelling `rg`/`fd`/`git`), not a mocked `Command`.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-git-test-{name}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp test dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create temp test file's parent dir");
        }
        fs::write(&path, contents).expect("write temp test file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `git args...` in `dir`, panicking on failure -- test-setup-only
/// (seeding commits/branches/stashes), not the crate's own `process::
/// run_action` (which never panics, since it also runs against whatever
/// real repo the user has open).
pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git").current_dir(dir).args(args).status().expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

/// `git init` plus the minimal identity config every commit needs --
/// without a global `user.name`/`user.email` (true in a sandboxed CI-like
/// environment), a bare `git commit` would fail, so every test that needs
/// a real commit calls this first.
pub fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "user.name", "Test"]);
}
