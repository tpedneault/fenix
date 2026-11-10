//! A document on a remote machine: opened and saved through whatever is
//! mounted for its host, never through this machine's disk.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};

use fenix_core::{path_key, Buffer, Cursor};
use fenix_vfs::{Kind, Meta, Over, Remote, Version};

/// A machine whose files are a map.
#[derive(Default)]
struct Machine {
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl Machine {
    fn meta(path: &str, bytes: &[u8]) -> Meta {
        let len = bytes.len() as u64;
        Meta { name: path.rsplit('/').next().unwrap_or_default().into(), kind: Kind::File, symlink: false, len, version: Version { len, mtime: 0 }, mode: 0o644 }
    }
}

impl Remote for Machine {
    fn home(&self) -> String {
        "/home/tom".into()
    }
    fn run(&self, _: &str, _: fenix_vfs::Run<'_>) -> io::Result<fenix_vfs::Ran> {
        Err(io::ErrorKind::Unsupported.into())
    }
    fn stat(&self, path: &str) -> io::Result<Meta> {
        self.files.lock().unwrap().get(path).map(|bytes| Machine::meta(path, bytes)).ok_or_else(|| io::ErrorKind::NotFound.into())
    }
    fn list(&self, _: &str) -> io::Result<Vec<Meta>> {
        Ok(Vec::new())
    }
    fn read(&self, path: &str) -> io::Result<(Vec<u8>, Meta)> {
        let files = self.files.lock().unwrap();
        let bytes = files.get(path).ok_or(io::ErrorKind::NotFound)?;
        Ok((bytes.clone(), Machine::meta(path, bytes)))
    }
    fn write(&self, path: &str, contents: &[u8], over: Over) -> io::Result<Meta> {
        let mut files = self.files.lock().unwrap();
        if over == Over::Nothing && files.contains_key(path) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        files.insert(path.to_string(), contents.to_vec());
        Ok(Machine::meta(path, contents))
    }
    fn rename(&self, _: &str, _: &str, _: bool) -> io::Result<()> {
        Ok(())
    }
    fn remove(&self, _: &str, _: bool) -> io::Result<()> {
        Ok(())
    }
    fn create_dir(&self, _: &str, _: bool) -> io::Result<()> {
        Ok(())
    }
    fn copy(&self, _: &str, _: &str) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_remote_document_is_read_edited_and_saved_on_its_machine() {
    let machine = Arc::new(Machine::default());
    machine.files.lock().unwrap().insert("/home/tom/Notes.txt".into(), b"one\r\ntwo\r\n".to_vec());
    fenix_vfs::mount("core-test", machine.clone());
    let path = fenix_vfs::remote("core-test", "/home/tom/Notes.txt");

    let mut buffer = Buffer::from_path(&path).unwrap();
    assert_eq!(buffer.path(), Some(path.as_path()), "the path is kept as it is");
    assert_eq!(path_key(buffer.path().unwrap()), path, "and keeps its case as a key");
    let mut cursor = Cursor::at_start();
    buffer.insert_str(&mut cursor, "zero ");
    assert!(buffer.is_dirty());
    buffer.save().unwrap();
    assert!(!buffer.is_dirty());
    let saved = machine.files.lock().unwrap()["/home/tom/Notes.txt"].clone();
    assert_eq!(String::from_utf8(saved).unwrap(), buffer.text());
    assert_eq!(buffer.read_disk_text().unwrap().0, buffer.text());

    // A new name there, which must not take another file's place.
    let mut new = Buffer::from_text("fresh");
    let taken = new.save_new(&path).unwrap_err();
    assert_eq!(taken.kind(), io::ErrorKind::AlreadyExists);
    let beside = fenix_vfs::remote("core-test", "/home/tom/new.txt");
    new.save_new(&beside).unwrap();
    assert_eq!(machine.files.lock().unwrap()["/home/tom/new.txt"], b"fresh");

    // Nothing of it ever touched this machine's disk.
    assert!(!path.exists());

    // With the machine gone, a save fails and the text stays unsaved.
    fenix_vfs::unmount("core-test");
    buffer.insert_str(&mut cursor, "more ");
    assert_eq!(buffer.save().unwrap_err().kind(), io::ErrorKind::NotConnected);
    assert!(buffer.is_dirty());
}
