//! Starting a session through the system's `ssh`.
//!
//! One `ssh` run starts the helper if it's there. If it isn't, the same
//! run says what the machine is, a second run puts the helper in place
//! and checks it arrived whole, and a third starts it.

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use sha2::{Digest, Sha256};

use crate::askpass::Askpass;
use crate::session::{Session, StartError};

/// A machine to connect to, as `ssh` names it.
#[derive(Debug, Clone)]
pub struct Target {
    /// `host`, `user@host`, or a `Host` of `~/.ssh/config`.
    pub host: String,
    /// More of `ssh`'s own arguments (`-p 2222`, `-o Key=value`).
    pub options: Vec<String>,
    /// The `ssh` to run.
    pub ssh: PathBuf,
}

impl Target {
    pub fn new(host: impl Into<String>) -> Target {
        Target { host: host.into(), options: Vec::new(), ssh: "ssh".into() }
    }

    /// `ssh … host remote`, with no terminal at either end.
    pub fn command(&self, remote: &str, askpass: Option<&Askpass>) -> io::Result<Command> {
        let mut command = Command::new(&self.ssh);
        command
            // No terminal there: the stream is the protocol's, byte for byte.
            .arg("-T")
            // A machine that doesn't answer is given up on, at the start
            // and later.
            .args(["-o", "ConnectTimeout=15", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3"])
            .args(&self.options)
            .arg(&self.host)
            .arg(remote);
        match askpass {
            Some(askpass) => askpass.hook(&mut command)?,
            // Nobody to ask: fail rather than wait on a question.
            None => {
                command.args(["-o", "BatchMode=yes"]);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Ok(command)
    }
}

/// The helpers Fenix ships: one static program per kind of machine,
/// named `fenix-remote-<os>-<arch>` in one folder.
#[derive(Debug, Clone)]
pub struct Helpers {
    pub folder: PathBuf,
}

impl Helpers {
    /// The helper for the machine `uname -sm` describes.
    fn for_system(&self, system: &str) -> Result<PathBuf, ConnectError> {
        let mut words = system.split_whitespace();
        let os = match words.next() {
            Some("Linux") => "linux",
            _ => return Err(ConnectError::Unsupported(system.to_string())),
        };
        let arch = match words.next() {
            Some("x86_64" | "amd64") => "x86_64",
            Some("aarch64" | "arm64") => "aarch64",
            _ => return Err(ConnectError::Unsupported(system.to_string())),
        };
        let path = self.folder.join(format!("fenix-remote-{os}-{arch}"));
        if path.is_file() {
            Ok(path)
        } else {
            Err(ConnectError::Unsupported(format!("{system}: {} isn't there", path.display())))
        }
    }
}

#[derive(Debug)]
pub enum ConnectError {
    /// `ssh` ended before a helper spoke: the host is unreachable, or
    /// refused the sign-in. What `ssh` said.
    Ssh(String),
    /// Fenix has no helper for this kind of machine.
    Unsupported(String),
    /// The helper couldn't be put in place, or didn't arrive whole.
    Install(String),
    Start(StartError),
    Io(io::Error),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::Ssh(said) => write!(f, "ssh couldn't connect: {}", said.trim()),
            ConnectError::Unsupported(system) => write!(f, "Fenix has no helper for {system}"),
            ConnectError::Install(why) => write!(f, "couldn't install Fenix's helper: {}", why.trim()),
            ConnectError::Start(error) => error.fmt(f),
            ConnectError::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<io::Error> for ConnectError {
    fn from(error: io::Error) -> Self {
        ConnectError::Io(error)
    }
}

/// A session, and how it came about.
pub struct Connected {
    pub session: Session,
    /// Whether the helper had to be put on the machine first.
    pub installed: bool,
}

/// Where the helper lives on the remote machine, under its home.
fn helper_folder() -> String {
    format!(".fenix/remote/{}", env!("CARGO_PKG_VERSION"))
}

/// Connects to `target` and starts the helper, installing it first if
/// the machine doesn't have this version's.
pub fn connect(target: &Target, helpers: &Helpers, askpass: Option<&Askpass>) -> Result<Connected, ConnectError> {
    match start(target, askpass) {
        Ok(session) => Ok(Connected { session, installed: false }),
        Err(ConnectError::Start(StartError::Missing(system))) => {
            install(target, &helpers.for_system(&system)?, askpass)?;
            Ok(Connected { session: start(target, askpass)?, installed: true })
        }
        Err(error) => Err(error),
    }
}

fn start(target: &Target, askpass: Option<&Askpass>) -> Result<Session, ConnectError> {
    // `exec`: the helper takes the shell's place, so that the end of the
    // connection is the end of the helper, with nothing in between.
    let remote = format!(
        r#"sh -c 'f="$HOME/{}/fenix-remote"; if [ -x "$f" ]; then exec "$f" --stdio; else echo "{}$(uname -sm)"; fi'"#,
        helper_folder(),
        fenix_remote::MISSING
    );
    let mut child = target.command(&remote, askpass)?.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (stdin, stdout, stderr) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    // What ssh says for itself: the reason, when it fails.
    let said = thread::spawn(move || {
        let mut said = String::new();
        let _ = { stderr }.read_to_string(&mut said);
        said
    });
    match Session::start(stdout, stdin) {
        Ok(session) => Ok(session.carried_by(child)),
        Err(StartError::Closed(printed)) => {
            let _ = child.wait();
            let said = said.join().unwrap_or_default();
            Err(ConnectError::Ssh(if said.trim().is_empty() { printed } else { said }))
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            Err(ConnectError::Start(error))
        }
    }
}

/// Copies the helper over `ssh` itself, to a name beside the real one,
/// and moves it into place only once all of it is there. The machine's
/// own count of what arrived has to match what was sent.
fn install(target: &Target, helper: &Path, askpass: Option<&Askpass>) -> Result<(), ConnectError> {
    let bytes = std::fs::read(helper)?;
    let sent = hex(&Sha256::digest(&bytes));
    // Counted there before it takes the helper's name: a copy cut short
    // never becomes the program a later connection runs.
    let remote = format!(
        r#"sh -c 'd="$HOME/{}"; n="$d/fenix-remote.new"; umask 077; mkdir -p "$d" && cat > "$n" && h=$( (sha256sum "$n" 2>/dev/null || shasum -a 256 "$n") | cut -d" " -f1) && echo "FENIX-SUM $h" && [ "$h" = "{sent}" ] && chmod 700 "$n" && mv -f "$n" "$d/fenix-remote"'"#,
        helper_folder()
    );
    let mut child = target.command(&remote, askpass)?.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let mut stdin = child.stdin.take().expect("piped");
    let feeding = thread::spawn(move || stdin.write_all(&bytes));
    let output = child.wait_with_output()?;
    let fed = feeding.join().unwrap_or_else(|_| Err(io::Error::other("the copy stopped")));
    let said = String::from_utf8_lossy(&output.stderr);
    let arrived = String::from_utf8_lossy(&output.stdout);
    // On a line Fenix can find among whatever the shell greets with.
    let arrived = arrived.lines().find_map(|line| line.trim().strip_prefix("FENIX-SUM ")).unwrap_or_default().trim();
    if output.status.success() && arrived.eq_ignore_ascii_case(&sent) {
        return Ok(());
    }
    if !arrived.is_empty() && !arrived.eq_ignore_ascii_case(&sent) {
        return Err(ConnectError::Install(format!("what arrived isn't what was sent ({arrived}, not {sent})")));
    }
    Err(ConnectError::Install(match fed {
        Err(error) if said.trim().is_empty() => error.to_string(),
        _ => said.into_owned(),
    }))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
