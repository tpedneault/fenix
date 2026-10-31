//! The OS clipboard, safe to use from more than one thread at a time.
//!
//! `arboard` opens the Windows clipboard with `OpenClipboard(NULL)` for
//! every operation and closes it afterwards. That call only keeps *other
//! processes* out: a second thread of this process opens it too, while
//! the first still has it open. The first thread's `CloseClipboard` then
//! closes it under the second, and user32 frees the handle the second
//! thread got from `GetClipboardData` while it is still reading it -- heap
//! corruption, and the process aborts with `STATUS_HEAP_CORRUPTION`.
//!
//! Fenix itself only touches the clipboard from its main thread, but
//! tests run many `App`s in parallel, each with its own clipboard (every
//! surface's extension reads it when it takes the keyboard). So every operation here
//! holds one process-wide lock, which is what `OpenClipboard` was assumed
//! to be.

use std::borrow::Cow;
use std::sync::{Mutex, MutexGuard};

static LOCK: Mutex<()> = Mutex::new(());

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `arboard::Clipboard` with each operation serialized process-wide.
pub struct Clipboard(arboard::Clipboard);

impl Clipboard {
    pub fn new() -> Result<Self, arboard::Error> {
        let _lock = lock();
        arboard::Clipboard::new().map(Clipboard)
    }

    pub fn get_text(&mut self) -> Result<String, arboard::Error> {
        let _lock = lock();
        self.0.get_text()
    }

    pub fn get_image(&mut self) -> Result<arboard::ImageData<'static>, arboard::Error> {
        let _lock = lock();
        self.0.get_image()
    }

    pub fn set_text<'a, T: Into<Cow<'a, str>>>(&mut self, text: T) -> Result<(), arboard::Error> {
        let _lock = lock();
        self.0.set_text(text)
    }

    pub fn set_image(&mut self, image: arboard::ImageData) -> Result<(), arboard::Error> {
        let _lock = lock();
        self.0.set_image(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Without the lock this aborts the whole test binary on Windows
    /// (`STATUS_HEAP_CORRUPTION`) rather than failing. Read-only, so the
    /// clipboard's contents are left alone.
    #[test]
    fn reading_the_clipboard_from_several_threads_at_once_is_safe() {
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(|| {
                    for _ in 0..300 {
                        if let Ok(mut clipboard) = Clipboard::new() {
                            let _ = clipboard.get_text();
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
