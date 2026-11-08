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
    /// Spawns the platform shell (`default_shell`: PowerShell 7 on Windows
    /// when it's installed, else Windows PowerShell; `$SHELL` -- falling
    /// back to `/bin/sh` -- elsewhere) into a fresh PTY sized
    /// `rows` x `cols`. Returns the `Terminal` plus the PTY's reader
    /// half separately: the reader needs to move into a background
    /// thread the *caller* owns (`fenix-gui`'s own reader thread,
    /// mirroring `DockerLogFollower`), not one this crate spawns itself.
    pub fn spawn(rows: u16, cols: u16) -> std::io::Result<(Terminal, Box<dyn Read + Send>)> {
        Self::spawn_in(rows, cols, None)
    }

    /// `spawn`, starting the shell in `cwd`.
    ///
    /// `None` inherits the editor's own working directory, which is
    /// what a terminal opened from nowhere in particular should do.
    /// Passing one is what makes "a shell *here*" mean here -- without
    /// it the answer is a shell somewhere else, and the first thing
    /// anybody types is a `cd`.
    pub fn spawn_in(rows: u16, cols: u16, cwd: Option<&std::path::Path>) -> std::io::Result<(Terminal, Box<dyn Read + Send>)> {
        Self::spawn_command(rows, cols, cwd, shell_command())
    }

    /// Runs `program args` in a fresh PTY instead of a shell -- for a
    /// terminal that *is* one tool (a serial monitor, a debugger console)
    /// rather than a place to type commands. Its exit is the terminal's
    /// exit, exactly as `exit` is for a shell.
    pub fn spawn_program(
        rows: u16,
        cols: u16,
        cwd: Option<&std::path::Path>,
        program: &str,
        args: &[String],
    ) -> std::io::Result<(Terminal, Box<dyn Read + Send>)> {
        let mut command = CommandBuilder::new(program);
        command.args(args);
        Self::spawn_command(rows, cols, cwd, command)
    }

    fn spawn_command(rows: u16, cols: u16, cwd: Option<&std::path::Path>, mut command: CommandBuilder) -> std::io::Result<(Terminal, Box<dyn Read + Send>)> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        let child = pair.slave.spawn_command(command).map_err(|err| std::io::Error::other(err.to_string()))?;
        // The slave end is only needed to spawn the child -- dropping it
        // here (rather than holding it in `Terminal`) matches portable-
        // pty's own documented usage, and avoids it holding the PTY open
        // past the child's own lifetime on platforms where that matters.
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(|err| std::io::Error::other(err.to_string()))?;
        // Off the caller's thread, which is the UI thread: a paste into
        // a shell that has stopped reading would otherwise hold the window.
        let writer = pair.master.take_writer().map_err(|err| std::io::Error::other(err.to_string()))?;
        let writer: Box<dyn Write + Send> = Box::new(fenix_rpc::QueueWriter::spawn("fenix-terminal-writer", writer)?);
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

    /// Pastes `text` as typed: its line breaks as Enter (`\r`), and --
    /// when the program asked for it (bracketed paste, `ESC [ ? 2004 h`)
    /// -- between the markers that tell it this is a paste, so a shell
    /// doesn't run a multi-line paste a line at a time. Escapes in it are
    /// dropped: a paste can't end the bracket early or drive the terminal.
    pub fn paste(&mut self, text: &str) -> std::io::Result<()> {
        let bytes = paste_bytes(text, self.parser.screen().bracketed_paste());
        self.write_input(&bytes)
    }

    /// The text from one cell to another (either way round), as rows of
    /// the screen shown -- scrolled back or not -- a row's trailing
    /// spaces dropped, a row the program wrapped joined to the next.
    pub fn text_between(&self, a: (u16, u16), b: (u16, u16)) -> String {
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        let text = self.parser.screen().contents_between(from.0, from.1, to.0, to.1.saturating_add(1));
        text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
    }

    /// Row `row` of the screen shown, as text.
    pub fn row_text(&self, row: u16) -> String {
        let (_, cols) = self.parser.screen().size();
        self.parser.screen().contents_between(row, 0, row, cols)
    }

    /// The folder the shell last said it's in (`OSC 7` / `OSC 9;9`), if
    /// it has.
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.parser.callbacks().cwd.as_deref()
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

/// The platform's own interactive shell, when `terminal.shell` doesn't
/// name one: on Windows PowerShell 7 (`pwsh`) when it's installed --
/// on `PATH` or where its installer puts it -- else Windows PowerShell
/// 5.1, which every Windows has; elsewhere `$SHELL`, falling back to
/// `/bin/sh`. PowerShell starts without its banner (`-NoLogo`): a
/// terminal opened beside the code is for the prompt, not the copyright.
pub fn default_shell() -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let on_path = std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("pwsh.exe").is_file()));
        let installed = std::env::var_os("ProgramFiles").map(|p| std::path::PathBuf::from(p).join("PowerShell").join("7").join("pwsh.exe")).filter(|p| p.is_file());
        let program = if on_path { "pwsh.exe".to_string() } else { installed.map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "powershell.exe".to_string()) };
        (program, vec!["-NoLogo".to_string()])
    }
    #[cfg(not(windows))]
    {
        (std::env::var("SHELL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "/bin/sh".to_string()), Vec::new())
    }
}

fn shell_command() -> CommandBuilder {
    let (program, args) = default_shell();
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command
}

/// What a paste of `text` sends: line breaks as `\r`, escapes dropped,
/// and the bracketed-paste markers around it when `bracketed`.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let clean: String = text.replace("\r\n", "\r").replace('\n', "\r").chars().filter(|&c| c != '\x1b').collect();
    if bracketed {
        [b"\x1b[200~".as_slice(), clean.as_bytes(), b"\x1b[201~"].concat()
    } else {
        clean.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A paste's lines end in Enter, it's bracketed when the program
    /// asked, and nothing in it can close the bracket early.
    #[test]
    fn a_paste_is_typed_bracketed_when_asked() {
        assert_eq!(paste_bytes("ls\nexit\r\n", false), b"ls\rexit\r");
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(paste_bytes("x\x1b[201~rm", true), b"\x1b[200~x[201~rm\x1b[201~");
    }

    #[test]
    fn a_shell_can_be_started_somewhere_in_particular() {
        // What makes "a shell *here*" mean here -- without it the first
        // thing anybody types is a `cd`.
        let dir = std::env::temp_dir().join(format!("fenix-terminal-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (mut term, _reader) = Terminal::spawn_in(24, 80, Some(&dir)).expect("failed to spawn a shell");
        assert!(term.is_alive());
        term.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shell_command_resolves_the_platform_default_shell() {
        let cmd = shell_command();
        let program = cmd.get_argv()[0].to_string_lossy().to_lowercase();
        #[cfg(windows)]
        assert!(program.contains("pwsh") || program.contains("powershell"), "expected PowerShell, got {program:?}");
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
        // On Windows, `cmd.exe`: the same ConPTY session and its opening
        // cursor-position query, without PowerShell 5.1's line editor,
        // which on a loaded CI runner stalls part-way through echoing
        // what's typed. (`shell_command_resolves_the_platform_default_shell`
        // keeps PowerShell as the default.)
        #[cfg(windows)]
        let (mut term, mut reader) = Terminal::spawn_program(24, 80, None, "cmd.exe", &[]).expect("failed to spawn cmd.exe");
        #[cfg(not(windows))]
        let (mut term, mut reader) = Terminal::spawn(24, 80).expect("failed to spawn a shell");

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

        // What the screen holds, read until `done` says so or `secs` pass.
        let mut last_text = String::new();
        let mut pump = |term: &mut Terminal, secs: u64, done: &dyn Fn(&str) -> bool| -> bool {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < deadline {
                let Ok(chunk) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) else { return false };
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
                let hit = done(&text);
                last_text = text;
                if hit {
                    return true;
                }
            }
            false
        };
        // Typed once the shell shows its prompt, and a key at a time, as a
        // person types.
        let prompted = |text: &str| text.trim_end().ends_with(['>', '$', '#', '%']);
        pump(&mut term, 60, &prompted);
        for key in b"echo fenix-terminal-test-marker\r" {
            term.write_input(&[*key]).expect("failed to write input");
            std::thread::sleep(Duration::from_millis(20));
        }
        let found = pump(&mut term, 60, &|text: &str| text.contains("fenix-terminal-test-marker"));
        if !found {
            eprintln!("fenix-terminal test: last observed screen text:\n{last_text}");
        }
        assert!(found, "expected the echoed marker to appear in the parsed screen within the timeout");
    }
}
