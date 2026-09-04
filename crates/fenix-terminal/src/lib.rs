//! A real, interactive terminal -- spawns the platform shell into a PTY
//! (`portable-pty`, ConPTY on Windows) and feeds its output into a
//! `vt100::Parser`, which maintains a queryable screen grid (cells with
//! characters + colors + attributes) for a host to render however it
//! wants. Deliberately has no thread/event-loop knowledge of its own --
//! same split this workspace already uses for `fenix-docker`/`fenix-git`
//! (pure shell/process logic) versus `fenix-gui` (owns the actual
//! background reader thread and `FenixUserEvent` wiring, mirroring
//! `DockerLogFollower`/`GitStatusPoller`).

use std::io::{Read, Write};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

mod query;

/// How many rows of scrollback `vt100` keeps beyond the live screen --
/// mouse-wheel scrolling (`Terminal::scroll`) has nothing to scroll
/// into without this.
const SCROLLBACK_LINES: usize = 10_000;

pub struct Terminal {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    parser: vt100::Parser<query::QueryReplies>,
}

impl Terminal {
    /// Spawns the platform shell (`powershell.exe` on Windows, `$SHELL`
    /// -- falling back to `/bin/sh` -- elsewhere) into a fresh PTY sized
    /// `rows` x `cols`. Returns the `Terminal` plus the PTY's reader
    /// half separately: the reader needs to move into a background
    /// thread the *caller* owns (`fenix-gui`'s own reader thread,
    /// mirroring `DockerLogFollower`), not one this crate spawns itself.
    pub fn spawn(rows: u16, cols: u16) -> std::io::Result<(Terminal, Box<dyn Read + Send>)> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        let child = pair.slave.spawn_command(shell_command()).map_err(|err| std::io::Error::other(err.to_string()))?;
        // The slave end is only needed to spawn the child -- dropping it
        // here (rather than holding it in `Terminal`) matches portable-
        // pty's own documented usage, and avoids it holding the PTY open
        // past the child's own lifetime on platforms where that matters.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|err| std::io::Error::other(err.to_string()))?;
        let writer = pair.master.take_writer().map_err(|err| std::io::Error::other(err.to_string()))?;
        let parser = vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK_LINES, query::QueryReplies::default());
        Ok((Terminal { child, writer, master: pair.master, parser }, reader))
    }

    /// Feeds raw PTY output bytes (from the caller's own reader thread)
    /// into the `vt100` parser, and immediately types back whatever
    /// that output *asked for* -- see the `query` module for why a
    /// terminal that only ever listens is a terminal that hangs.
    ///
    /// Answering here, rather than handing the replies to the caller,
    /// is what makes this impossible to forget: on Windows the session
    /// produces nothing at all until the very first query is answered,
    /// so a host that skipped the step would see an empty panel and a
    /// live shell with no hint of why.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        let replies = self.parser.callbacks_mut().take();
        if !replies.is_empty() {
            // A failed write means the shell is gone, which `is_alive`
            // is already the place to notice -- there is nothing useful
            // to do about it from inside a render-path call.
            let _ = self.write_input(&replies);
        }
    }

    /// Writes raw input bytes (already encoded, e.g. by a caller's own
    /// key-to-terminal-sequence mapping) to the shell.
    pub fn write_input(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Resizes both the real PTY and the parser's own screen model --
    /// both need to agree, or the shell's own idea of `$COLUMNS`/
    /// `$LINES` drifts from what's actually being rendered.
    pub fn resize(&mut self, rows: u16, cols: u16) -> std::io::Result<()> {
        self.master
            .resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        self.parser.screen_mut().set_size(rows, cols);
        Ok(())
    }

    /// `false` once the shell has exited (e.g. the user typed `exit`) --
    /// lets the caller respawn a fresh shell instead of showing a dead
    /// panel the next time it's opened.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Moves the scrollback view by `delta` rows -- positive scrolls
    /// back into history, negative scrolls toward the live bottom.
    /// `vt100::Screen::set_scrollback` already clamps to however much
    /// history actually exists; this only needs to keep the *offset*
    /// itself non-negative before handing it off.
    pub fn scroll(&mut self, delta: isize) {
        let screen = self.parser.screen_mut();
        let target = (screen.scrollback() as isize + delta).max(0) as usize;
        screen.set_scrollback(target);
    }

    /// Kills the shell -- lets a caller's own reader thread (blocked on
    /// the PTY's own blocking `Read::read`, with no way to attach a
    /// timeout to that call) unblock via EOF/error, the same "kill to
    /// force the blocking read to return" shape this workspace already
    /// uses for `DockerLogFollower`'s own teardown.
    ///
    /// Deliberately does *not* call `Child::wait()`, which blocks with
    /// an OS-level *infinite* timeout -- polls `try_wait()` with a
    /// bounded retry loop instead, so teardown can never hang even if
    /// the kill signal is slow to take effect.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        for _ in 0..20 {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(_) => return,
            }
        }
    }
}

/// `powershell.exe` on Windows; `$SHELL` (falling back to `/bin/sh`)
/// elsewhere -- the platform's own default interactive shell, not a
/// configurable choice in v1.
fn shell_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        CommandBuilder::new("powershell.exe")
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        CommandBuilder::new(shell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn shell_command_resolves_the_platform_default_shell() {
        let cmd = shell_command();
        let program = cmd.get_argv()[0].to_string_lossy().to_lowercase();
        #[cfg(windows)]
        assert!(program.contains("powershell"), "expected powershell, got {program:?}");
        #[cfg(not(windows))]
        assert!(!program.is_empty());
    }

    /// A real end-to-end round trip: spawn the real platform shell,
    /// type a command, and confirm its output actually lands in the
    /// parsed screen -- consistent with this project's own established
    /// "test against the real thing" posture for external-process code
    /// (`fenix-git`/`fenix-docker`'s own tests spawn real `git`/`docker`
    /// rather than mocking).
    ///
    /// This is the test that pins the bug in `query`. It used to be
    /// `#[ignore]`d, on the theory that a shell taking 90 seconds to
    /// print its first prompt was antivirus scanning a freshly spawned
    /// child. It was not: the session emitted its four-byte cursor-
    /// position query and then waited, forever, for an answer nobody
    /// was giving it. No timeout would ever have been long enough.
    /// Un-ignored now that the answer is sent -- a real `powershell.exe`
    /// gets from spawn to echoed output in a few seconds here, most of
    /// it the user's own profile loading.
    ///
    /// `Read::read` on a PTY blocks until *some* data arrives, with no
    /// way to attach a timeout to the call itself -- so the actual
    /// reading happens on its own thread, forwarding each chunk through
    /// a channel, and only the channel's `recv_timeout` enforces a real
    /// bound; a naive "loop read() until a deadline" doesn't actually
    /// bound anything, since a single call that never returns would
    /// hang the loop regardless of the deadline check around it.
    #[test]
    fn spawn_write_and_read_a_real_shell_round_trip() {
        let (mut term, mut reader) = Terminal::spawn(24, 80).expect("failed to spawn a shell");
        term.write_input(b"echo fenix-terminal-test-marker\r").expect("failed to write input");

        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => return,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        });

        let deadline = Instant::now() + Duration::from_secs(90);
        let mut found = false;
        let mut last_text = String::new();
        while Instant::now() < deadline {
            let Ok(chunk) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else { break };
            term.process(&chunk);
            let screen = term.screen();
            let (rows, cols) = screen.size();
            let mut text = String::new();
            for row in 0..rows {
                for col in 0..cols {
                    if let Some(cell) = screen.cell(row, col) {
                        text.push_str(cell.contents());
                    }
                }
            }
            last_text = text.clone();
            if text.contains("fenix-terminal-test-marker") {
                found = true;
                break;
            }
        }
        if !found {
            eprintln!("fenix-terminal test: last observed screen text:\n{last_text}");
        }
        assert!(found, "expected the echoed marker to appear in the parsed screen within the timeout");
    }
}
