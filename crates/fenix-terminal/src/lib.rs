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
    /// Rows that have gone up into the history since the shell started
    /// (`top_row`).
    scrolled: u64,
    /// What it runs, as its file is named: `pwsh`, `bash`.
    program: String,
    /// The folder it started in.
    start_dir: Option<std::path::PathBuf>,
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
        let program = command.get_argv().first().map(|p| program_name(&p.to_string_lossy())).unwrap_or_default();
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        // None is the editor's own folder: given none, portable-pty would
        // start the program in the user's home instead.
        let start_dir = cwd.map(std::path::Path::to_path_buf).or_else(|| std::env::current_dir().ok());
        if let Some(cwd) = &start_dir {
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
        Ok((Terminal { child, writer, master: pair.master, parser, scrolled: 0, program, start_dir }, reader))
    }

    /// The folder it started in.
    pub fn start_dir(&self) -> Option<&std::path::Path> {
        self.start_dir.as_deref()
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
        let before = self.history_len();
        self.parser.process(bytes);
        let after = self.history_len();
        // Once the history is full it stays the same length as rows go up
        // through it; then the rows that went are the ones that came in.
        self.scrolled += if after > before { (after - before) as u64 } else if after == SCROLLBACK_LINES && before == SCROLLBACK_LINES { bytes.iter().filter(|&&b| b == b'\n').count() as u64 } else { 0 };
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
    ///
    /// A program that didn't ask for bracketed paste in a session that
    /// takes Windows key events (`win32_input`) -- PowerShell's line editor
    /// under ConPTY, which never asks -- gets its line breaks as Shift-Enter,
    /// PSReadLine's new line: the paste lands whole, to be run with Enter.
    pub fn paste(&mut self, text: &str) -> std::io::Result<()> {
        let bytes = paste_bytes(text, self.parser.screen().bracketed_paste(), self.win32_input());
        self.write_input(&bytes)
    }

    /// Whether the session takes keys as Windows key events
    /// (win32-input-mode, which ConPTY asks for): then `win32_key` can
    /// send the keys VT has no bytes of its own for.
    pub fn win32_input(&self) -> bool {
        self.parser.callbacks().win32_input
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

    /// What the terminal runs, as its file is named: `pwsh`, `bash`.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Whether a command is running in it, when its shell marks its
    /// prompts (`OSC 133`); None when it doesn't say.
    pub fn running(&self) -> Option<bool> {
        self.parser.callbacks().running
    }

    /// How many rows the history holds: the most the view scrolls back.
    fn history_len(&mut self) -> usize {
        let screen = self.parser.screen_mut();
        let at = screen.scrollback();
        screen.set_scrollback(usize::MAX);
        let len = screen.scrollback();
        screen.set_scrollback(at);
        len
    }

    /// The first row shown, counted from the first the shell ever printed
    /// -- a place in everything it's printed that stays the same as more
    /// comes in and the view scrolls. Row `r` shown is `top_row() + r`.
    pub fn top_row(&self) -> u64 {
        self.scrolled.saturating_sub(self.parser.screen().scrollback() as u64)
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
        let args = shell_args(&program, vec!["-NoLogo".to_string()]);
        (program, args)
    }
    #[cfg(not(windows))]
    {
        (std::env::var("SHELL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "/bin/sh".to_string()), Vec::new())
    }
}

/// PowerShell's prompt, wrapped to say where it is first (`OSC 9;9`, as
/// Windows Terminal's shell integration has it): stock PowerShell never
/// says, so after a `cd` a relative path it prints -- a compiler's
/// `src\main.rs:2:18` -- would be read from where it started. Whatever
/// prompt the profile set is kept; no double quotes, so the command line
/// carries it as it is.
const POWERSHELL_CWD_PROMPT: &str = "$global:__fenix_prompt = $function:prompt; function global:prompt { $p = & $global:__fenix_prompt; if ($PWD.Provider.Name -eq 'FileSystem') { -join ([char]27, ']9;9;', [char]34, $PWD.ProviderPath, [char]34, [char]27, [char]92, $p) } else { $p } }";

/// A shell's arguments, with what lets Fenix follow it: PowerShell
/// (`pwsh`, `powershell`) started for a prompt -- not running a command or
/// a script of its own -- reports its folder at each prompt
/// (`POWERSHELL_CWD_PROMPT`). Any other shell's are as they are.
pub fn shell_args(program: &str, mut args: Vec<String>) -> Vec<String> {
    let name = program_name(program).to_ascii_lowercase();
    let runs_something = args.iter().any(|a| {
        let a = a.to_ascii_lowercase();
        let a = a.trim_start_matches(['-', '/']);
        !a.is_empty() && ["command", "c", "file", "f", "encodedcommand", "ec", "e"].contains(&a)
    });
    if (name == "pwsh" || name == "powershell") && !runs_something {
        args.extend(["-NoExit".to_string(), "-Command".to_string(), POWERSHELL_CWD_PROMPT.to_string()]);
    }
    args
}

fn shell_command() -> CommandBuilder {
    let (program, args) = default_shell();
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command
}

/// A program's name from its path: its file name, `.exe` dropped.
fn program_name(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".exe") { name[..name.len() - 4].to_string() } else { name.to_string() }
}

/// What a paste of `text` sends: line breaks as `\r`, escapes dropped,
/// and the bracketed-paste markers around it when `bracketed`. Not
/// bracketed in a session taking Windows key events (`win32`), its line
/// breaks are Shift-Enter instead, so a line editor that binds it (as
/// PSReadLine does, to a new line) doesn't run the paste a line at a time.
fn paste_bytes(text: &str, bracketed: bool, win32: bool) -> Vec<u8> {
    let clean: String = text.replace("\r\n", "\r").replace('\n', "\r").chars().filter(|&c| c != '\x1b').collect();
    if bracketed {
        [b"\x1b[200~".as_slice(), clean.as_bytes(), b"\x1b[201~"].concat()
    } else if win32 && clean.contains('\r') {
        let shift_enter = win32_key(0x0D, 0x1C, '\r', SHIFT_PRESSED);
        let mut out = Vec::new();
        for (i, line) in clean.split('\r').enumerate() {
            if i > 0 {
                out.extend_from_slice(&shift_enter);
            }
            out.extend_from_slice(line.as_bytes());
        }
        out
    } else {
        clean.into_bytes()
    }
}

/// Windows' `SHIFT_PRESSED` control-key state.
pub const SHIFT_PRESSED: u32 = 0x0010;
/// Windows' `LEFT_CTRL_PRESSED` control-key state.
pub const LEFT_CTRL_PRESSED: u32 = 0x0008;

/// A key pressed and let go as win32-input-mode spells it --
/// `ESC [ Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`, the virtual key, scan code,
/// character, down or up, control-key state and repeat count -- which
/// ConPTY turns into the console key events a Windows program reads:
/// Ctrl-Space as PSReadLine's `Ctrl+Spacebar`, not the NUL VT has for it.
pub fn win32_key(vk: u16, scan: u16, ch: char, state: u32) -> Vec<u8> {
    let ch = ch as u32;
    format!("\x1b[{vk};{scan};{ch};1;{state};1_\x1b[{vk};{scan};{ch};0;{state};1_").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// As output scrolls rows up into the history, `top_row` counts them,
    /// and scrolling the view back moves it the other way: a row's place
    /// in everything printed stays the same.
    #[cfg(unix)]
    #[test]
    fn top_row_counts_the_rows_gone_up_into_the_history() {
        let (mut term, mut reader) = Terminal::spawn_program(10, 40, None, "sh", &["-c".to_string(), "seq 1 25; sleep 2".to_string()]).expect("sh");
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline && !term.screen().contents().contains("25") {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => term.process(&buf[..n]),
            }
        }
        // 25 lines and the cursor's row on a 10-row screen: 16 went up.
        assert_eq!(term.top_row(), 16, "{}", term.screen().contents());
        assert_eq!(term.row_text(0).trim(), "17");
        term.scroll(5);
        assert_eq!(term.top_row(), 11, "scrolled back five");
        assert_eq!(term.row_text(0).trim(), "12");
        term.kill();
    }

    #[test]
    fn a_programs_name_is_its_file_name() {
        assert_eq!(program_name(r"C:\Program Files\PowerShell\7\pwsh.exe"), "pwsh");
        assert_eq!(program_name("powershell.exe"), "powershell");
        assert_eq!(program_name("/bin/bash"), "bash");
    }

    /// A paste's lines end in Enter, it's bracketed when the program
    /// asked, and nothing in it can close the bracket early.
    #[test]
    fn a_paste_is_typed_bracketed_when_asked() {
        assert_eq!(paste_bytes("ls\nexit\r\n", false, false), b"ls\rexit\r");
        assert_eq!(paste_bytes("a\nb", true, false), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(paste_bytes("x\x1b[201~rm", true, false), b"\x1b[200~x[201~rm\x1b[201~");
        assert_eq!(paste_bytes("a\nb", true, true), b"\x1b[200~a\rb\x1b[201~", "bracketed when asked, win32 or not");
    }

    /// Under ConPTY, with no bracketed paste asked for, a paste's line
    /// breaks are Shift-Enter, so PowerShell takes it whole; one line is
    /// sent as it is.
    #[test]
    fn a_paste_into_a_windows_line_editor_breaks_lines_with_shift_enter() {
        let shift_enter = "\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_";
        assert_eq!(paste_bytes("echo one\r\necho two", false, true), format!("echo one{shift_enter}echo two").into_bytes());
        assert_eq!(paste_bytes("echo one", false, true), b"echo one");
    }

    /// PowerShell started for a prompt reports its folder; one running a
    /// command or script of its own, and other shells, are left as named.
    #[test]
    fn powershell_is_started_to_say_where_it_is() {
        let args = shell_args(r"C:\Program Files\PowerShell\7\pwsh.exe", vec!["-NoLogo".to_string()]);
        assert_eq!(&args[..3], ["-NoLogo", "-NoExit", "-Command"]);
        assert!(args[3].contains("]9;9;") && !args[3].contains('"'), "{}", args[3]);
        assert_eq!(shell_args("powershell.exe", vec![]).len(), 3);
        assert_eq!(shell_args("pwsh", vec!["-c".to_string(), "dir".to_string()]), ["-c", "dir"]);
        assert_eq!(shell_args("pwsh", vec!["-File".to_string(), "x.ps1".to_string()]), ["-File", "x.ps1"]);
        assert_eq!(shell_args("bash", vec!["-l".to_string()]), ["-l"]);
        assert_eq!(shell_args("cmd.exe", vec![]), Vec::<String>::new());
    }

    /// A terminal started nowhere in particular starts in the editor's own
    /// folder, not the user's home.
    #[test]
    fn a_terminal_given_no_folder_starts_in_the_editors() {
        let (mut term, _reader) = Terminal::spawn_in(24, 80, None).expect("a shell");
        assert_eq!(term.start_dir(), std::env::current_dir().ok().as_deref());
        term.kill();
    }

    /// The real PowerShell, after a `cd`, says where it went.
    #[cfg(windows)]
    #[test]
    fn powershell_says_where_it_is_after_a_cd() {
        let (program, args) = default_shell();
        let (mut term, mut reader) = Terminal::spawn_program(24, 120, None, &program, &args).expect("PowerShell");
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
        });
        let dir = std::env::temp_dir().join(format!("fenix-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pump = |term: &mut Terminal, secs: u64, done: &dyn Fn(&Terminal) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while !done(term) && Instant::now() < deadline {
                if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
                    term.process(&chunk);
                }
            }
        };
        pump(&mut term, 60, &|t| t.cwd().is_some());
        assert!(term.cwd().is_some(), "the first prompt says where it is: {}", term.screen().contents());
        term.write_input(format!("cd '{}'\r", dir.display()).as_bytes()).unwrap();
        let want = dir.clone();
        pump(&mut term, 30, &|t| t.cwd().is_some_and(|c| c.as_os_str().eq_ignore_ascii_case(want.as_os_str())));
        let got = term.cwd().map(std::path::Path::to_path_buf);
        term.kill();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(got.as_deref().is_some_and(|c| c.as_os_str().eq_ignore_ascii_case(dir.as_os_str())), "{got:?}");
    }

    #[test]
    fn a_win32_key_is_its_press_and_release() {
        assert_eq!(win32_key(0x20, 0x39, ' ', LEFT_CTRL_PRESSED), b"\x1b[32;57;32;1;8;1_\x1b[32;57;32;0;8;1_");
    }

    /// ConPTY asks for Windows key events as a session starts, which is
    /// what lets Ctrl-Space and a whole paste reach PowerShell.
    #[cfg(windows)]
    #[test]
    fn conpty_asks_for_windows_key_events() {
        let (mut term, mut reader) = Terminal::spawn_program(24, 80, None, "cmd.exe", &[]).expect("cmd.exe");
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while !term.win32_input() && Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
                term.process(&chunk);
            }
        }
        let asked = term.win32_input();
        term.kill();
        assert!(asked, "ConPTY didn't ask for win32-input-mode");
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
