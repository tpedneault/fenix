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
