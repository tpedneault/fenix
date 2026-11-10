//! The protocol between Fenix and `fenix-remote`, its helper on the far
//! end of an SSH connection.
//!
//! The helper's stdin and stdout are the connection. It first writes one
//! line, [`BANNER`] and a JSON [`Hello`], so that whatever a login shell
//! printed before it can be told apart and skipped; from then on both
//! ways are [`Frame`]s.
//!
//! A frame is `u32 length (big endian) · u8 kind · u32 id · payload`,
//! the length counting the kind, id and payload. The id is the request's:
//! every answer, and every piece of a started program's output, carries
//! the id of the request that asked.

pub mod server;

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// The protocol's version. Fenix and the helper are of one release, so
/// this only has to catch a stale helper left running or misplaced.
pub const PROTOCOL: u32 = 1;

/// What the helper's first line starts with.
pub const BANNER: &str = "FENIX-REMOTE ";

/// What the starting command prints instead when there's no helper to
/// run, followed by `uname -sm`.
pub const MISSING: &str = "FENIX-MISSING ";

/// The largest frame either side accepts.
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

/// How much of a file travels in one frame. Small enough that other
/// requests get a word in between two pieces of a large file, and that a
/// transfer can be stopped soon after it's asked to.
pub const PIECE: usize = 256 * 1024;

/// What the helper prints, run as `--print-env`: its environment, which
/// is the user's login shell's when that shell ran it.
pub const ENVIRONMENT: &str = "FENIX-ENV ";

/// What a frame is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Fenix asks: a JSON [`Request`].
    Request = 1,
    /// Bytes that go with the request of the same id: a piece of a
    /// file's new contents, or of a program's stdin.
    Body = 2,
    /// No more of them: the file is whole, the program's stdin ends.
    BodyEnd = 3,
    /// Stop: the program and everything it started, or the transfer.
    Kill = 4,
    /// The answer, as JSON.
    Ok = 16,
    /// The request failed: a JSON [`Failure`].
    Err = 17,
    /// A piece of the answer, as bytes: a file's contents, in order,
    /// before the `Ok` that ends them.
    Bytes = 18,
    /// What a program wrote to stdout.
    Stdout = 19,
    /// What a program wrote to stderr.
    Stderr = 20,
    /// A program ended: a JSON [`Exit`]. Always after its last output.
    Exit = 21,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Kind> {
        Some(match byte {
            1 => Kind::Request,
            2 => Kind::Body,
            3 => Kind::BodyEnd,
            4 => Kind::Kill,
            16 => Kind::Ok,
            17 => Kind::Err,
            18 => Kind::Bytes,
            19 => Kind::Stdout,
            20 => Kind::Stderr,
            21 => Kind::Exit,
            _ => return None,
        })
    }
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: Kind,
    pub id: u32,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(kind: Kind, id: u32, payload: impl Into<Vec<u8>>) -> Frame {
        Frame { kind, id, payload: payload.into() }
    }

    /// A frame whose payload is `value` as JSON.
    pub fn json(kind: Kind, id: u32, value: &impl Serialize) -> Frame {
        Frame { kind, id, payload: serde_json::to_vec(value).expect("protocol types serialize") }
    }

    /// Writes the frame. The caller flushes.
    pub fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let length = u32::try_from(self.payload.len() + 5).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
        // One write: a frame is never two packets on a socket.
        let mut bytes = Vec::with_capacity(self.payload.len() + 9);
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.push(self.kind as u8);
        bytes.extend_from_slice(&self.id.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        out.write_all(&bytes)
    }

    /// Reads the next frame; `None` at a clean end of the stream.
    pub fn read(input: &mut impl Read) -> io::Result<Option<Frame>> {
        let mut head = [0u8; 9];
        match input.read(&mut head[..1])? {
            0 => return Ok(None),
            _ => input.read_exact(&mut head[1..])?,
        }
        let length = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        if !(5..=MAX_FRAME + 5).contains(&length) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {length} bytes")));
        }
        let kind = Kind::from_byte(head[4]).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("frame kind {}", head[4])))?;
        let id = u32::from_be_bytes([head[5], head[6], head[7], head[8]]);
        let mut payload = vec![0u8; length - 5];
        input.read_exact(&mut payload)?;
        Ok(Some(Frame { kind, id, payload }))
    }
}

/// The helper's first words.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub protocol: u32,
    /// The helper's own version: Fenix's.
    pub version: String,
    /// `linux`, `macos`, `windows`.
    pub os: String,
    /// `x86_64`, `aarch64`.
    pub arch: String,
    /// The user's home folder there.
    pub home: String,
    pub user: String,
}

/// What Fenix asks for. Paths are the remote's own, absolute, with `/`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// One file or folder → `Ok` [`Entry`].
    Stat { path: String },
    /// A folder's entries → `Ok` `Vec<Entry>`.
    List { path: String },
    /// A file's contents → `Bytes` frames, then `Ok` [`Entry`] as it was
    /// when the read began. `Kill` stops it.
    Read { path: String },
    /// Replaces a file's contents with the `Body` frames that follow, up
    /// to `BodyEnd`, through a temporary file beside it → `Ok` [`Entry`]
    /// as written. With `expect`, only if the file is still as Fenix last
    /// saw it; with `create_new`, only if there is no such file. `Kill`
    /// stops it and leaves the file as it was.
    Write {
        path: String,
        #[serde(default)]
        expect: Option<Version>,
        #[serde(default)]
        create_new: bool,
    },
    /// Moves or renames → `Ok`. Without `replace`, refuses to take the
    /// place of something there.
    Rename {
        from: String,
        to: String,
        #[serde(default)]
        replace: bool,
    },
    /// Deletes a file, a link, or an empty folder; with `recursive`, a
    /// folder and all in it → `Ok`. There is no bin to take it back from.
    Remove {
        path: String,
        #[serde(default)]
        recursive: bool,
    },
    /// Makes a folder; with `parents`, those above it too, and no
    /// complaint if it's there → `Ok` [`Entry`].
    CreateDir {
        path: String,
        #[serde(default)]
        parents: bool,
    },
    /// Copies a file, or a folder and all in it, on the remote machine
    /// itself → `Ok`. Refuses to take the place of something there.
    Copy { from: String, to: String },
    /// Starts a program → `Stdout`/`Stderr` frames, then `Exit`. `Err`
    /// if it couldn't be started.
    Exec(Exec),
}

/// A program to run.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Exec {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Whether stdin will be sent (`Body`, `BodyEnd`). Without it the
    /// program's stdin is closed from the start.
    #[serde(default)]
    pub stdin: bool,
}

/// What tells one state of a file from the next.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Version {
    pub len: u64,
    /// Nanoseconds since the Unix epoch.
    pub mtime: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    File,
    Dir,
    Other,
}

/// A file or folder.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    /// The name alone for a listing, the path asked for otherwise.
    pub name: String,
    /// What it is, a link followed to its end.
    pub kind: FileKind,
    pub symlink: bool,
    pub len: u64,
    pub mtime: i64,
    /// Unix permission bits; 0 where there are none.
    pub mode: u32,
}

impl Entry {
    pub fn version(&self) -> Version {
        Version { len: self.len, mtime: self.mtime }
    }
}

/// Why a request failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Failure {
    pub code: FailureCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    NotFound,
    Denied,
    /// A checked write found the file changed since Fenix read it.
    Changed,
    /// Something is already there, where nothing was to be replaced.
    Exists,
    /// Stopped because Fenix asked.
    Cancelled,
    Other,
}

/// How a program ended.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Exit {
    /// Its exit code; `None` when a signal ended it.
    pub code: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_survives_the_wire() {
        let frame = Frame::json(Kind::Request, 7, &Request::Read { path: "/etc/hosts".into() });
        let mut wire = Vec::new();
        frame.write(&mut wire).unwrap();
        Frame::new(Kind::Body, 8, vec![0u8, 255, 10]).write(&mut wire).unwrap();
        let mut input = wire.as_slice();
        assert_eq!(Frame::read(&mut input).unwrap(), Some(frame));
        assert_eq!(Frame::read(&mut input).unwrap().unwrap().payload, vec![0u8, 255, 10]);
        assert_eq!(Frame::read(&mut input).unwrap(), None);
    }

    #[test]
    fn a_frame_too_large_or_of_no_known_kind_is_refused() {
        let mut huge = Vec::from(u32::MAX.to_be_bytes());
        huge.extend([1, 0, 0, 0, 0]);
        assert!(Frame::read(&mut huge.as_slice()).is_err());
        let unknown = [0, 0, 0, 5, 99, 0, 0, 0, 0];
        assert!(Frame::read(&mut unknown.as_slice()).is_err());
    }

    #[test]
    fn a_stream_cut_inside_a_frame_is_an_error_not_an_end() {
        let mut wire = Vec::new();
        Frame::new(Kind::Body, 1, vec![1, 2, 3]).write(&mut wire).unwrap();
        wire.pop();
        assert!(Frame::read(&mut wire.as_slice()).is_err());
    }
}
