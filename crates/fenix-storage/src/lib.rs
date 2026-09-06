//! Checked, staged writes shared by documents, settings, and recovery.
//!
//! The destination is never truncated. A complete, flushed and synced sibling
//! file replaces it only after serialization succeeds. This protects against
//! interrupted writes; it does not promise survival of every hardware failure.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;

pub fn write(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write(path, |writer| writer.write_all(contents))
}

pub fn atomic_write(
    path: &Path,
    serialize: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    atomic_write_impl(path, serialize, false)
}

/// Create a new document without ever replacing an existing destination.
pub fn atomic_write_new(
    path: &Path,
    serialize: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> io::Result<()> {
    atomic_write_impl(path, serialize, true)
}

fn atomic_write_impl(
    path: &Path,
    serialize: impl FnOnce(&mut dyn Write) -> io::Result<()>,
    new_only: bool,
) -> io::Result<()> {
    // Follow existing symlinks so saving through a link updates its target.
    let target = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path)?,
        Ok(_) => path.to_path_buf(),
        Err(err) if err.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(err) => return Err(err),
    };
    let metadata = match fs::metadata(&target) {
        Ok(meta) => {
            if new_only {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "destination already exists",
                ));
            }
            if !meta.is_file() || meta.permissions().readonly() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "destination is not a writable file",
                ));
            }
            Some(meta)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(err),
    };
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut staged = tempfile::Builder::new()
        .prefix(".fenix-save-")
        .tempfile_in(parent)?;
    {
        let mut writer = BufWriter::new(staged.as_file_mut());
        serialize(&mut writer)?;
        writer.flush()?;
    }
    if let Some(meta) = &metadata {
        staged.as_file().set_permissions(meta.permissions())?;
    }
    staged.as_file().sync_all()?;

    // Close our handle before Windows opens the replacement for exclusive access.
    let staged = staged.into_temp_path();

    #[cfg(windows)]
    if metadata.is_some() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
        let destination: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        let source: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
        // Preserve the destination's Windows ACL and metadata. Do not fall
        // back to delete-and-rename if sharing or permission rules reject it.
        let replaced = unsafe {
            ReplaceFileW(
                destination.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        return if replaced != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        };
    }

    if metadata.is_some() {
        staged.persist(&target).map_err(|err| err.error)?;
    } else {
        // Do not overwrite a file created by somebody else after our check.
        staged.persist_noclobber(&target).map_err(|err| err.error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creating_a_document_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("existing.txt");
        fs::write(&path, "original").unwrap();
        assert!(atomic_write_new(&path, |writer| writer.write_all(b"replacement")).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "original");
    }

    #[test]
    fn concurrent_creation_during_serialization_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.txt");
        let result = atomic_write_new(&path, |writer| {
            writer.write_all(b"ours")?;
            fs::write(&path, "theirs")
        });
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "theirs");
    }

    #[test]
    fn failed_serialization_preserves_destination_and_cleans_staging_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        fs::write(&path, "original").unwrap();
        let result = atomic_write(&path, |writer| {
            writer.write_all(&vec![b'x'; 32_000])?;
            Err(io::Error::other("injected write failure"))
        });
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn saves_under_unicode_paths_with_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("work space 日本語 é");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("résumé 文書.txt");
        write(&path, "before\r\n".as_bytes()).unwrap();
        write(&path, "after 😀\r\n".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "after 😀\r\n");
        assert_eq!(fs::read_dir(parent).unwrap().count(), 1);
    }

    #[test]
    fn creates_and_replaces_with_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        write(&path, b"first\r\n").unwrap();
        write(&path, b"second\r\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second\r\n");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn refuses_readonly_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("readonly.txt");
        fs::write(&path, "original").unwrap();
        let original = fs::metadata(&path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        let result = write(&path, b"replacement");
        fs::set_permissions(&path, original).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }

    #[cfg(windows)]
    #[test]
    fn sharing_violation_preserves_destination() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.txt");
        fs::write(&path, "original").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(write(&path, b"replacement").is_err());
        drop(lock);
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    }
}
