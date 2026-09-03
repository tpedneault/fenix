//! `Content-Length`-framed message transport, shared by `fenix-lsp` and
//! (later) `fenix-dap` -- both protocols frame every message the same
//! way over a child process's stdio: a `Content-Length: N\r\n` header,
//! a blank line, then exactly `N` bytes of UTF-8 JSON with no trailing
//! newline of its own. Deliberately generic over plain `BufRead`/`Write`
//! rather than tied to a specific process-spawning mechanism, and
//! deliberately ignorant of JSON itself (hands back/takes raw bytes) --
//! LSP and DAP each have their own message shapes (`fenix-lsp` depends
//! on the `lsp-types` crate for its; DAP's are chosen when that crate is
//! built), and this crate has no business knowing either.

use std::io::{self, BufRead, Write};

/// On Windows, `std::process::Command::new` only launches a bare program
/// name via `CreateProcess`'s own loader, which -- unlike a real shell --
/// never consults `PATHEXT`. A server/adapter installed as an npm-style
/// `.cmd` shim (`typescript-language-server`, `bash-language-server`,
/// ... -- extremely common for JS-ecosystem tooling) fails to spawn at
/// all with a bare "program not found" this way, even though a terminal
/// launches the exact same bare name fine. Fixed by searching `PATH` the
/// same way `cmd.exe` would -- every directory, every extension
/// `PATHEXT` lists (falling back to the common default if unset) -- and
/// handing `Command` the resolved full path instead, which Rust's own
/// Windows `Command` implementation already knows how to run correctly,
/// `.cmd`/`.bat` script included (internally via `cmd.exe /c`,
/// transparent to the caller). Shared by `fenix-lsp` and `fenix-dap`
/// (both spawn a child process by a possibly-bare command name the same
/// way) rather than duplicated -- this bug class has nothing to do with
/// LSP or DAP specifically, it's purely a Windows process-spawning gap.
///
/// A command that's already a specific path (has an extension, or
/// contains a path separator) is returned unchanged -- nothing to
/// resolve, and forcing it through this search would only risk *not*
/// finding the exact file the caller named. A no-op on every other
/// platform: Unix has no such gap, since a script's shebang line makes
/// it directly executable with no shell involved either way.
#[cfg(windows)]
pub fn resolve_command(command: &str) -> String {
    if std::path::Path::new(command).extension().is_some() || command.contains(['/', '\\']) {
        return command.to_string();
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    let extensions: Vec<&str> = pathext.split(';').filter(|e| !e.is_empty()).collect();
    let Some(path_var) = std::env::var_os("PATH") else { return command.to_string() };
    for dir in std::env::split_paths(&path_var) {
        for ext in &extensions {
            let candidate = dir.join(format!("{command}{ext}"));
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    command.to_string() // not found anywhere -- let Command::new's own error surface as usual
}

#[cfg(not(windows))]
pub fn resolve_command(command: &str) -> String {
    command.to_string()
}

/// Strips Windows' `\\?\` "verbatim" prefix, if present -- `std::fs::
/// canonicalize` always adds it (`C:\Users\...` canonicalizes to
/// `\\?\C:\Users\...`), but it's a Windows API implementation detail
/// (opts out of the usual 260-character path length limit and any
/// further normalization) that has no place in a URI, a DAP `Source`
/// path, or whatever path representation the rest of an app compares/
/// hashes against -- a server/adapter would never send it back, so a
/// caller that canonicalizes a path on one side of a comparison and not
/// the other (or takes this prefix along unstripped into a message a
/// protocol peer has to match against its own idea of "this file") gets
/// two representations of the same file that don't match. Confirmed
/// live on both sides this crate serves: `fenix-lsp`'s `path_to_uri`
/// (a malformed `file:////?/C:/...` URI broke diagnostics path-
/// matching) and `fenix-dap`'s own breakpoint `Source.path` (`debugpy`
/// silently never matched a breakpoint registered under the verbatim-
/// prefixed form against the plain-form path it reports for a running
/// frame -- the breakpoint looked "verified" in the `setBreakpoints`
/// response but was never actually hit). A no-op on every other
/// platform (and on a Windows path that was never canonicalized in the
/// first place).
pub fn normalize(path: std::path::PathBuf) -> std::path::PathBuf {
    match path.to_str() {
        Some(s) => match s.strip_prefix(r"\\?\") {
            Some(stripped) => std::path::PathBuf::from(stripped),
            None => path,
        },
        None => path,
    }
}

/// Reads one framed message from `reader`, returning its raw body bytes
/// (not parsed -- see this module's own doc comment for why).
///
/// `Ok(None)` means a clean close: the far end's process exited (or its
/// pipe was dropped) before sending another message, with zero bytes of
/// a new header read yet -- the ordinary, expected way a connection
/// ends. Any *other* truncation (EOF partway through a header line, or
/// partway through the body) is `Err`, not `Ok(None)`: that's a
/// genuinely malformed or killed-mid-message connection, which a caller
/// should treat as a real error rather than silently as "nothing more
/// to read."
pub fn read_message<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return if content_length.is_none() {
                Ok(None)
            } else {
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed mid-header"))
            };
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            // The blank line separating headers from the body -- but
            // only once at least one Content-Length has actually been
            // seen; a stray blank line before any header is malformed,
            // not a (nonsensical) zero-length message.
            if content_length.is_some() {
                break;
            }
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message body started before any Content-Length header"));
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            let value = value.trim();
            let parsed = value
                .parse::<usize>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, format!("malformed Content-Length header: {value:?}")))?;
            content_length = Some(parsed);
        }
        // Any other header (e.g. `Content-Type`, which both LSP and DAP
        // technically allow) is read and silently ignored -- neither
        // protocol requires a client to act on one.
    }
    let content_length = content_length.expect("loop only breaks once content_length is Some");
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Writes one framed message: the `Content-Length` header block, then
/// `body` verbatim. `body` must already be well-formed JSON bytes --
/// this function only frames it, it never parses or validates it.
pub fn write_message<W: Write>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(all(test, windows))]
mod resolve_command_tests {
    use super::resolve_command;

    #[test]
    fn a_command_that_already_names_an_extension_is_returned_unchanged() {
        assert_eq!(resolve_command("pyright-langserver.exe"), "pyright-langserver.exe");
        assert_eq!(resolve_command("some-tool.cmd"), "some-tool.cmd");
    }

    #[test]
    fn a_command_that_is_already_a_path_is_returned_unchanged() {
        assert_eq!(resolve_command("C:\\tools\\clangd"), "C:\\tools\\clangd");
        assert_eq!(resolve_command("./bin/clangd"), "./bin/clangd");
    }

    #[test]
    fn a_bare_name_found_nowhere_on_path_falls_back_to_itself() {
        assert_eq!(resolve_command("definitely-not-a-real-lsp-server-binary-xyz"), "definitely-not-a-real-lsp-server-binary-xyz");
    }
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize;
    use std::path::PathBuf;

    #[test]
    fn strips_windows_verbatim_prefix() {
        assert_eq!(normalize(PathBuf::from(r"\\?\C:\Users\thoma\file.py")), PathBuf::from(r"C:\Users\thoma\file.py"));
    }

    #[test]
    fn is_a_no_op_on_a_path_with_no_verbatim_prefix() {
        assert_eq!(normalize(PathBuf::from(r"C:\Users\thoma\file.py")), PathBuf::from(r"C:\Users\thoma\file.py"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_a_single_well_formed_message() {
        let raw = b"Content-Length: 13\r\n\r\n{\"foo\":\"bar\"}";
        let mut cursor = Cursor::new(&raw[..]);
        let msg = read_message(&mut cursor).unwrap();
        assert_eq!(msg, Some(b"{\"foo\":\"bar\"}".to_vec()));
    }

    #[test]
    fn reads_two_consecutive_messages_off_the_same_stream() {
        let raw = b"Content-Length: 4\r\n\r\ntrue\
                    Content-Length: 5\r\n\r\nfalse";
        let mut cursor = Cursor::new(&raw[..]);
        assert_eq!(read_message(&mut cursor).unwrap(), Some(b"true".to_vec()));
        assert_eq!(read_message(&mut cursor).unwrap(), Some(b"false".to_vec()));
        assert_eq!(read_message(&mut cursor).unwrap(), None);
    }

    #[test]
    fn ignores_headers_other_than_content_length() {
        let raw = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\nContent-Length: 2\r\n\r\n{}";
        let mut cursor = Cursor::new(&raw[..]);
        assert_eq!(read_message(&mut cursor).unwrap(), Some(b"{}".to_vec()));
    }

    #[test]
    fn tolerates_extra_whitespace_around_the_length_value() {
        let raw = b"Content-Length:    2   \r\n\r\n{}";
        let mut cursor = Cursor::new(&raw[..]);
        assert_eq!(read_message(&mut cursor).unwrap(), Some(b"{}".to_vec()));
    }

    #[test]
    fn a_clean_close_before_any_header_bytes_is_a_normal_none() {
        let mut cursor = Cursor::new(&b""[..]);
        assert_eq!(read_message(&mut cursor).unwrap(), None);
    }

    #[test]
    fn eof_partway_through_the_header_block_is_an_error_not_none() {
        // A Content-Length line with no trailing blank-line terminator
        // before the stream just stops.
        let raw = b"Content-Length: 2\r\n";
        let mut cursor = Cursor::new(&raw[..]);
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn truncated_body_is_an_error() {
        let raw = b"Content-Length: 10\r\n\r\n{}"; // header promises 10 bytes, only 2 follow
        let mut cursor = Cursor::new(&raw[..]);
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn missing_content_length_header_is_an_error() {
        let raw = b"Content-Type: text/plain\r\n\r\n{}";
        let mut cursor = Cursor::new(&raw[..]);
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn malformed_content_length_value_is_an_error() {
        let raw = b"Content-Length: not-a-number\r\n\r\n{}";
        let mut cursor = Cursor::new(&raw[..]);
        let err = read_message(&mut cursor).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn write_message_frames_exactly_per_the_spec() {
        let mut buf = Vec::new();
        write_message(&mut buf, b"{\"a\":1}").unwrap();
        assert_eq!(buf, b"Content-Length: 7\r\n\r\n{\"a\":1}".to_vec());
    }

    #[test]
    fn write_then_read_round_trips_the_same_body() {
        let mut buf = Vec::new();
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        write_message(&mut buf, body).unwrap();
        let mut cursor = Cursor::new(buf.as_slice());
        assert_eq!(read_message(&mut cursor).unwrap(), Some(body.to_vec()));
    }

    #[test]
    fn an_empty_body_round_trips_correctly() {
        let mut buf = Vec::new();
        write_message(&mut buf, b"").unwrap();
        let mut cursor = Cursor::new(buf.as_slice());
        assert_eq!(read_message(&mut cursor).unwrap(), Some(Vec::new()));
    }
}
