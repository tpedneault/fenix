use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A real, uniquely-named directory under the OS temp dir, removed on
/// drop. `fenix-fs`'s whole surface is "do real things to a real
/// filesystem," so its tests exercise a real filesystem too, the same
/// way `fenix-core`'s tests use a real `Rope` rather than a mock buffer.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-fs-test-{name}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp test dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn touch(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, b"").expect("touch temp test file");
        path
    }

    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, contents).expect("write temp test file");
        path
    }

    pub fn mkdir(&self, name: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir(&path).expect("mkdir temp test dir");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
