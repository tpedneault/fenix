//! Converts between filesystem paths and the `file://` URIs LSP uses to
//! identify every document. `lsp_types::Uri` (a thin wrapper around
//! `fluent_uri::Uri<String>`) deliberately has no path <-> URI
//! conversion of its own -- it's inherently platform-specific (a Windows
//! drive letter needs a leading `/` before it in the URI, but not once
//! parsed back out; which characters need percent-encoding is a URI
//! concern a path type has no reason to know about) -- so this crate
//! provides it.

use std::path::{Path, PathBuf};

/// Strips Windows' `\\?\` "verbatim" prefix -- moved to `fenix_rpc`
/// (see its own doc comment) once `fenix-dap`'s breakpoint `Source`
/// paths turned out to need the exact same fix as this crate's own
/// `path_to_uri`: this is a shared "talking to a protocol peer about a
/// filesystem path" concern, not an LSP-specific one. Re-exported here
/// so every existing call site in this crate (and in `fenix-gui`, which
/// calls it as `fenix_lsp::normalize`) keeps working unchanged.
pub use fenix_rpc::normalize;

/// Converts an absolute filesystem path to a `file://` URI. Returns
/// `None` if `path` isn't absolute -- a server needs an unambiguous
/// location, and there's no such thing as a relative URI a document
/// could be meaningfully identified by.
pub fn path_to_uri(path: &Path) -> Option<lsp_types::Uri> {
    if !path.is_absolute() {
        return None;
    }
    let path = normalize(path.to_path_buf());
    let slash_path = path.to_string_lossy().replace('\\', "/");
    // A Windows absolute path's own string form doesn't start with `/`
    // (it starts with the drive letter, e.g. `C:/Users/...`) -- `file://`
    // needs one more `/` there than a Unix path already has, so the
    // authority (empty, between the two `file://` slashes) is followed
    // by an absolute path starting with `/`.
    let slash_path = if slash_path.starts_with('/') { slash_path } else { format!("/{slash_path}") };

    let mut encoded = String::from("file://");
    for segment in slash_path.split('/') {
        if segment.is_empty() {
            continue;
        }
        encoded.push('/');
        encoded.push_str(&percent_encode_segment(segment));
    }
    if encoded == "file://" {
        // The root itself (`path` was exactly `/` or a bare drive root).
        encoded.push('/');
    }
    encoded.parse().ok()
}

/// The inverse: a `file://` URI back to a filesystem path. `None` if
/// `uri` isn't a `file://` URI at all (LSP allows other schemes in
/// principle, e.g. `untitled:` for an unsaved buffer some clients
/// create; this crate never generates those, but a server could in
/// theory send one back).
pub fn uri_to_path(uri: &lsp_types::Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    let decoded = percent_decode(rest);
    #[cfg(windows)]
    {
        // `/C:/Users/...` -> `C:/Users/...`: strip the leading slash
        // that exists only so the URI has an absolute path (starting
        // with `/`) after its empty authority -- `path_to_uri`'s own
        // doc comment explains why it's there in the first place.
        let decoded = decoded.strip_prefix('/').unwrap_or(&decoded);
        Some(PathBuf::from(decoded.replace('/', "\\")))
    }
    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

/// Percent-encodes everything in `segment` outside RFC 3986's
/// `unreserved`/`sub-delims`/`:`/`@` sets (the characters a URI path
/// segment, i.e. `pchar`, allows unescaped) -- notably including every
/// non-ASCII byte, encoded one UTF-8 byte at a time, which is the
/// simplest rule that's unambiguously correct for every character
/// rather than trying to special-case which non-ASCII text a URI parser
/// happens to accept raw.
fn percent_encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        let sub_delims_etc = matches!(byte, b':' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=');
        if unreserved || sub_delims_etc {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `normalize`'s own tests now live in `fenix_rpc` (see its own doc
    // comment for why it moved there).

    #[cfg(windows)]
    #[test]
    fn path_to_uri_strips_the_verbatim_prefix_a_real_canonicalize_call_adds() {
        // The actual bug this exists to catch: `std::fs::canonicalize`
        // on Windows always returns a `\\?\`-prefixed path, and without
        // stripping it first, the resulting URI would contain that
        // prefix verbatim (`file:////?/C:/...`, since the prefix's own
        // leading `\\` becomes `//` before this function's own "does it
        // already start with /" check even runs) instead of a normal
        // one a real server would recognize or ever send back.
        let dir = std::env::temp_dir();
        let file = dir.join(format!("fenix-lsp-uri-test-{}.txt", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let canonical = std::fs::canonicalize(&file).unwrap();
        assert!(canonical.to_string_lossy().starts_with(r"\\?\"), "test assumption: canonicalize adds the verbatim prefix");

        let uri = path_to_uri(&canonical).unwrap();
        assert!(!uri.as_str().contains("?"), "URI must not contain the verbatim-prefix marker: {}", uri.as_str());
        assert_eq!(uri_to_path(&uri), Some(normalize(canonical)));
        std::fs::remove_file(&file).ok();
    }

    #[test]
    fn a_relative_path_has_no_uri() {
        assert!(path_to_uri(Path::new("relative/path.py")).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn a_windows_drive_path_gets_a_leading_slash_before_the_drive_letter() {
        let uri = path_to_uri(Path::new(r"C:\Users\thoma\file.py")).unwrap();
        assert_eq!(uri.as_str(), "file:///C:/Users/thoma/file.py");
    }

    #[cfg(windows)]
    #[test]
    fn round_trips_a_windows_path_through_uri_and_back() {
        let path = PathBuf::from(r"C:\Users\thoma\file.py");
        let uri = path_to_uri(&path).unwrap();
        assert_eq!(uri_to_path(&uri), Some(path));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_unix_absolute_path_becomes_a_triple_slash_uri() {
        let uri = path_to_uri(Path::new("/home/user/file.py")).unwrap();
        assert_eq!(uri.as_str(), "file:///home/user/file.py");
    }

    #[cfg(not(windows))]
    #[test]
    fn round_trips_a_unix_path_through_uri_and_back() {
        let path = PathBuf::from("/home/user/file.py");
        let uri = path_to_uri(&path).unwrap();
        assert_eq!(uri_to_path(&uri), Some(path));
    }

    #[test]
    fn a_space_in_the_path_is_percent_encoded_and_decodes_back_correctly() {
        let path = if cfg!(windows) { PathBuf::from(r"C:\My Projects\a b.py") } else { PathBuf::from("/home/my user/a b.py") };
        let uri = path_to_uri(&path).unwrap();
        assert!(uri.as_str().contains("%20"), "expected percent-encoded space, got {}", uri.as_str());
        assert!(!uri.as_str().contains(' '), "URI must not contain a literal space: {}", uri.as_str());
        assert_eq!(uri_to_path(&uri), Some(path));
    }

    #[test]
    fn a_windows_drive_letters_colon_is_not_percent_encoded() {
        if cfg!(windows) {
            let uri = path_to_uri(Path::new(r"C:\file.py")).unwrap();
            assert!(uri.as_str().contains("C:"), "drive-letter colon should be preserved raw: {}", uri.as_str());
        }
    }

    #[test]
    fn a_non_ascii_character_round_trips_correctly() {
        let path = if cfg!(windows) { PathBuf::from(r"C:\café\résumé.py") } else { PathBuf::from("/home/café/résumé.py") };
        let uri = path_to_uri(&path).unwrap();
        assert_eq!(uri_to_path(&uri), Some(path));
    }

    #[test]
    fn a_non_file_uri_returns_none_from_uri_to_path() {
        let uri: lsp_types::Uri = "untitled:Untitled-1".parse().unwrap();
        assert_eq!(uri_to_path(&uri), None);
    }
}
