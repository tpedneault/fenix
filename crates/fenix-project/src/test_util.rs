use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A real, uniquely-named directory under the OS temp dir, removed on
/// drop -- `fenix-project`'s surface is real filesystem/process
/// interaction, so its tests exercise a real filesystem, the same
/// reasoning `fenix-explorer`'s own `TempDir` already established.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("fenix-project-test-{name}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp test dir");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn write(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs for temp test file");
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

/// Held by every test that sets the extensions' project kinds or asks
/// what's declared: the registry is the process's, and tests run side by
/// side.
pub static KINDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `KINDS`, whatever a test that panicked holding it left.
pub fn kinds_lock() -> std::sync::MutexGuard<'static, ()> {
    KINDS.lock().unwrap_or_else(|e| e.into_inner())
}
