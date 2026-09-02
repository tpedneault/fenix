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
