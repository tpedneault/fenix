//! Answering the terminal *queries* a program (or the platform's own
//! pseudoconsole) sends upstream, expecting the emulator to type a
//! reply back on the input side.
//!
//! This is not a nicety. On Windows, ConPTY opens every session by
//! emitting a Device Status Report cursor-position query (`ESC [ 6 n`)
//! and **waits for the answer before letting the shell produce a single
//! further byte**. An emulator that renders output but never replies
//! deadlocks on its own first four bytes: the shell is alive, the pipe
//! is open, and nothing ever comes out of it. That was the whole of
//! Fenix's "the terminal panel shows nothing" bug -- not a slow spawn,
//! as an earlier round of this code assumed, but a conversation where
//! one side never spoke.
//!
//! Handled here rather than at the call site because these queries come
//! from *any* program in the session, not just the pseudoconsole's
//! opening handshake -- editors, pagers and TUIs all probe the terminal
//! this way, and a program that gets no answer either hangs or falls
//! back to a dumber rendering mode.

/// What Fenix claims to be when asked (Primary Device Attributes,
/// `ESC [ c`): a VT100 with the Advanced Video Option.
///
/// Deliberately modest. `vt100`, the parser behind the screen model,
/// implements a VT100/xterm *subset*; claiming to be a modern xterm
/// invites programs to send sequences that would then be silently
/// dropped and render as corruption. Answering honestly gets a program
/// to pick a feature set that actually works here.
const PRIMARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[?1;2c";

/// Secondary Device Attributes (`ESC [ > c`): terminal type 0 (VT100),
/// firmware version 10, no cartridge ROM -- the same honest answer as
/// `PRIMARY_DEVICE_ATTRIBUTES`, in the shape that query expects.
const SECONDARY_DEVICE_ATTRIBUTES: &[u8] = b"\x1b[>0;10;0c";

/// Collects the bytes owed back to the session in reply to queries seen
/// in its output. Plugged into `vt100::Parser` as its callbacks, so
/// every query is noticed in the same pass that renders the output
/// around it; `Terminal::process` drains this and writes it to the PTY.
#[derive(Default)]
pub(crate) struct QueryReplies {
    pending: Vec<u8>,
    /// The folder the shell last said it's in, when it says: `OSC 7`
    /// (`file://host/path`, what bash, zsh and fish integrations send)
    /// or `OSC 9;9` (`"C:\\path"`, Windows Terminal's, which a PowerShell
    /// prompt can send). A path in the output is read from there.
    pub(crate) cwd: Option<std::path::PathBuf>,
    /// Whether a command is running, when the shell marks its prompts
    /// (`OSC 133`, FinalTerm's marks that VS Code's and Windows
    /// Terminal's shell integrations send): `C` a command started, `A`
    /// a prompt and `D` a command done. None from a shell that doesn't.
    pub(crate) running: Option<bool>,
    /// Whether the session asked for keys as Windows key events
    /// (`ESC [ ? 9001 h`, win32-input-mode): ConPTY does, to be told keys
    /// VT has no bytes for -- Ctrl-Space, Shift-Enter -- as the console
    /// gives them.
    pub(crate) win32_input: bool,
}

/// The path in an `OSC 7` file URL: its host dropped, `%xx` decoded, and
/// on Windows the slash before a drive letter (`/C:/x`) too.
fn path_of_file_url(url: &str) -> Option<std::path::PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    let drive = path.len() > 2 && path.as_bytes()[2] == b':' && path.as_bytes()[1].is_ascii_alphabetic();
    Some(std::path::PathBuf::from(if drive { &path[1..] } else { &path[..] }))
}

impl QueryReplies {
    /// Takes everything owed so far, leaving the buffer empty.
    pub(crate) fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }
}

impl vt100::Callbacks for QueryReplies {
    /// Every query this cares about arrives here: `vt100` handles the
    /// CSI sequences that *change the screen* itself and hands the rest
    /// on, and a query changes nothing -- it asks.
    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, _i2: Option<u8>, params: &[&[u16]], c: char) {
        // Every one of these takes a single numeric argument, defaulting
        // to 0 when omitted (`ESC [ n` and `ESC [ 0 n` mean the same
        // thing) -- which is also what a bare `ESC [ c` relies on.
        let arg = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        // A private mode `vt100` doesn't know: win32-input-mode's on or off.
        if i1 == Some(b'?') && matches!(c, 'h' | 'l') && params.iter().any(|p| p.first() == Some(&9001)) {
            self.win32_input = c == 'h';
        }
        match (i1, c, arg) {
            // DSR: "are you all right?" -- yes, no malfunction.
            (None, 'n', 5) => self.pending.extend_from_slice(b"\x1b[0n"),
            // DSR: "where is the cursor?" -- the query ConPTY blocks on.
            // Reported 1-based, as the protocol counts rows and columns.
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.pending.extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // DECXCPR, the same question with the page number attached.
            // Always page 1: there is only ever one screen here.
            (Some(b'?'), 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.pending.extend_from_slice(format!("\x1b[?{};{};1R", row + 1, col + 1).as_bytes());
            }
            (None, 'c', 0) => self.pending.extend_from_slice(PRIMARY_DEVICE_ATTRIBUTES),
            (Some(b'>'), 'c', _) => self.pending.extend_from_slice(SECONDARY_DEVICE_ATTRIBUTES),
            _ => {}
        }
    }

    /// The shell saying where it is (`OSC 7`, `OSC 9;9`).
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        let text = |p: &[u8]| String::from_utf8_lossy(p).into_owned();
        match params {
            [b"7", url, ..] => {
                if let Some(path) = path_of_file_url(&text(url)) {
                    self.cwd = Some(path);
                }
            }
            [b"133", mark, ..] => match mark.first() {
                Some(b'C') => self.running = Some(true),
                Some(b'A' | b'D') => self.running = Some(false),
                _ => {}
            },
            [b"9", b"9", path, ..] => {
                let path = text(path);
                let path = path.trim().trim_matches('"');
                if !path.is_empty() {
                    self.cwd = Some(std::path::PathBuf::from(path));
                }
            }
            _ => {}
        }
    }

    /// `ESC Z` (DECID) -- the pre-CSI spelling of "identify yourself",
    /// still sent by older programs, and answered identically.
    fn unhandled_escape(&mut self, _: &mut vt100::Screen, i1: Option<u8>, _i2: Option<u8>, b: u8) {
        if i1.is_none() && b == b'Z' {
            self.pending.extend_from_slice(PRIMARY_DEVICE_ATTRIBUTES);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `bytes` through a real parser and returns whatever the
    /// session is owed in reply -- the same path `Terminal::process`
    /// takes, minus the PTY.
    fn replies_to(bytes: &[u8]) -> Vec<u8> {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, QueryReplies::default());
        parser.process(bytes);
        parser.callbacks_mut().take()
    }

    #[test]
    fn a_cursor_position_query_is_answered_with_the_real_cursor_position() {
        // The exact exchange ConPTY opens every Windows session with,
        // and refuses to proceed past.
        assert_eq!(replies_to(b"\x1b[6n"), b"\x1b[1;1R");
    }

    #[test]
    fn the_reported_position_follows_the_cursor_rather_than_being_a_fixed_answer() {
        // Programs use this to find out where they are (a shell drawing
        // a prompt, a pager measuring wrap); a constant answer would be
        // worse than none.
        assert_eq!(replies_to(b"hello\x1b[6n"), b"\x1b[1;6R");
        assert_eq!(replies_to(b"one\r\ntwo\x1b[6n"), b"\x1b[2;4R");
    }

    #[test]
    fn a_status_query_reports_no_malfunction() {
        assert_eq!(replies_to(b"\x1b[5n"), b"\x1b[0n");
    }

    #[test]
    fn the_extended_cursor_position_query_answers_with_a_page_number() {
        assert_eq!(replies_to(b"\x1b[?6n"), b"\x1b[?1;1;1R");
    }

    #[test]
    fn device_attribute_queries_are_answered_in_all_three_spellings() {
        assert_eq!(replies_to(b"\x1b[c"), PRIMARY_DEVICE_ATTRIBUTES);
        assert_eq!(replies_to(b"\x1b[0c"), PRIMARY_DEVICE_ATTRIBUTES);
        assert_eq!(replies_to(b"\x1bZ"), PRIMARY_DEVICE_ATTRIBUTES);
        assert_eq!(replies_to(b"\x1b[>c"), SECONDARY_DEVICE_ATTRIBUTES);
        assert_eq!(replies_to(b"\x1b[>0c"), SECONDARY_DEVICE_ATTRIBUTES);
    }

    #[test]
    fn ordinary_output_owes_nothing_back() {
        // The common case by a wide margin: replying to output that
        // asked nothing would inject junk into the shell's stdin.
        assert!(replies_to(b"just some text\r\n\x1b[31mand a color\x1b[m").is_empty());
    }

    #[test]
    fn several_queries_in_one_chunk_are_all_answered_in_order() {
        // A single read from the PTY can carry a whole handshake.
        let expected = [b"\x1b[1;1R".as_slice(), b"\x1b[0n", PRIMARY_DEVICE_ATTRIBUTES].concat();
        assert_eq!(replies_to(b"\x1b[6n\x1b[5n\x1b[c"), expected);
    }

    #[test]
    fn a_query_split_across_two_chunks_is_still_answered() {
        // Nothing guarantees a PTY read lands on an escape-sequence
        // boundary, and the parser is the thing that carries state
        // across chunks -- so the reply has to come from it, not from
        // scanning each chunk on its own.
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, QueryReplies::default());
        parser.process(b"\x1b[");
        assert!(parser.callbacks_mut().take().is_empty(), "nothing to answer yet -- the query isn't complete");
        parser.process(b"6n");
        assert_eq!(parser.callbacks_mut().take(), b"\x1b[1;1R");
    }

    /// Where the shell says it is: an `OSC 7` URL, or Windows Terminal's
    /// `OSC 9;9`; neither is owed a reply.
    #[test]
    fn the_shell_saying_where_it_is_is_kept() {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, QueryReplies::default());
        parser.process(b"\x1b]7;file://host/home/me/My%20Project\x07");
        assert_eq!(parser.callbacks().cwd.as_deref(), Some(std::path::Path::new("/home/me/My Project")));
        parser.process(b"\x1b]9;9;\"C:\\src\\fenix\"\x1b\\");
        assert_eq!(parser.callbacks().cwd.as_deref(), Some(std::path::Path::new("C:\\src\\fenix")));
        parser.process(b"\x1b]7;file://host/C:/Users/me\x07");
        assert_eq!(parser.callbacks().cwd.as_deref(), Some(std::path::Path::new("C:/Users/me")), "no slash before a drive");
        assert!(parser.callbacks_mut().take().is_empty());
    }

    /// A shell that marks its prompts says when a command runs.
    #[test]
    fn prompt_marks_say_when_a_command_runs() {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, QueryReplies::default());
        assert_eq!(parser.callbacks().running, None, "no marks: not known");
        parser.process(b"\x1b]133;A\x07$ \x1b]133;B\x07make\r\n\x1b]133;C\x07");
        assert_eq!(parser.callbacks().running, Some(true));
        parser.process(b"done\r\n\x1b]133;D;0\x07");
        assert_eq!(parser.callbacks().running, Some(false));
    }

    /// ConPTY asking for Windows key events is noticed, among other modes,
    /// and owed no reply.
    #[test]
    fn win32_input_mode_is_noticed() {
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, QueryReplies::default());
        assert!(!parser.callbacks().win32_input);
        parser.process(b"\x1b[?9001h\x1b[?1004h");
        assert!(parser.callbacks().win32_input);
        parser.process(b"\x1b[?25;9001l");
        assert!(!parser.callbacks().win32_input);
        assert!(parser.callbacks_mut().take().is_empty());
    }

    #[test]
    fn taking_the_replies_clears_them() {
        // They are written to the shell once; a second write would be
        // read as the user typing the escape sequence.
        let mut parser = vt100::Parser::new_with_callbacks(24, 80, 0, QueryReplies::default());
        parser.process(b"\x1b[6n");
        assert!(!parser.callbacks_mut().take().is_empty());
        assert!(parser.callbacks_mut().take().is_empty());
    }
}
