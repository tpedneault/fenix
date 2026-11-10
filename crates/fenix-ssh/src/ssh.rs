//! Starting a session through the system's `ssh`.
//!
//! One `ssh` run starts the helper if it's there. If it isn't, the same
//! run says what the machine is and takes the helper for it on its
//! stdin, counts what arrived and puts it in place; a second run starts
//! it.

use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use sha2::{Digest, Sha256};

use crate::askpass::Askpass;
use crate::session::{preamble, Session, StartError};

/// The line the remote machine's count of the helper is on.
const SUM: &str = "FENIX-SUM ";

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
            // The remote machine gets a connection for Fenix's helper and
            // nothing of this one's: not the agent's keys to use, not a
            // display to draw on, whatever `~/.ssh/config` says for the
            // host. (`ssh` keeps the first value it's given, so an option
            // above, which only a caller of this code can set, still
            // wins.)
            .args(["-o", "ForwardAgent=no", "-o", "ForwardX11=no"])
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
    /// The helpers shipped with this program: `FENIX_REMOTE_HELPERS` if
    /// it's set (a build from source has none beside it), else the
    /// `remote` folder beside the program, else, for a package with the
    /// program in `/usr/bin`, Fenix's own folder (`/usr/lib/fenix/remote`).
    pub fn shipped() -> Helpers {
        if let Some(folder) = std::env::var_os("FENIX_REMOTE_HELPERS").filter(|folder| !folder.is_empty()) {
            return Helpers { folder: folder.into() };
        }
        let beside = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.to_path_buf()));
        let packaged = beside.as_deref().and_then(Path::parent).map(|prefix| prefix.join("lib").join("fenix").join("remote"));
        let beside = beside.map(|folder| folder.join("remote"));
        match (beside, packaged) {
            (Some(beside), Some(packaged)) if !beside.is_dir() && packaged.is_dir() => Helpers { folder: packaged },
            (Some(beside), _) => Helpers { folder: beside },
            _ => Helpers { folder: "remote".into() },
        }
    }

    /// The helper for the machine `uname -sm` describes.
    fn for_system(&self, system: &str) -> Result<PathBuf, ConnectError> {
        let name = helper_name(system).ok_or_else(|| ConnectError::Unsupported(system.to_string()))?;
        let path = self.folder.join(name);
        if path.is_file() {
            Ok(path)
        } else {
            Err(ConnectError::Unsupported(format!("{system}: {} isn't there", path.display())))
        }
    }
}

fn helper_name(system: &str) -> Option<String> {
    let mut words = system.split_whitespace();
    let os = match words.next()? {
        "Linux" => "linux",
        _ => return None,
    };
    let arch = match words.next()? {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => return None,
    };
    Some(format!("fenix-remote-{os}-{arch}"))
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

impl ConnectError {
    /// Whether trying again by itself could help: the machine or the
    /// network may come back, a refused sign-in won't change its mind.
    pub fn worth_retrying(&self) -> bool {
        match self {
            ConnectError::Ssh(said) => !["Permission denied", "Host key verification failed", "Too many authentication failures"].iter().any(|refusal| said.contains(refusal)),
            ConnectError::Io(_) => true,
            ConnectError::Unsupported(_) | ConnectError::Install(_) | ConnectError::Start(_) => false,
        }
    }
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

/// What `ssh` is asked to run. With a helper there, it becomes the
/// helper (`exec`), so the end of the connection is the end of the
/// helper with nothing in between. Without one, it says what the machine
/// is, then waits for a helper on stdin: the count it should have on the
/// first line, the program after. The helper only takes its name once
/// all of it is there and counted right, so a copy cut short never
/// becomes the program a later connection runs. Then the versions before
/// it go.
fn script() -> String {
    format!(
        r#"sh -c 'r="$HOME/.fenix/remote"; d="$r/{version}"; f="$d/fenix-remote"; if [ -x "$f" ]; then exec "$f" --stdio; fi; echo "{missing}$(uname -sm)"; n="$f.new"; umask 077; mkdir -p "$d" && IFS= read -r want && cat > "$n" && h=$( (sha256sum "$n" 2>/dev/null || shasum -a 256 "$n") | cut -d" " -f1) && echo "{sum}$h" && [ "$h" = "$want" ] && chmod 700 "$n" && mv -f "$n" "$f" && for o in "$r"/*; do [ "$o" = "$d" ] || rm -rf "$o"; done'"#,
        version = env!("CARGO_PKG_VERSION"),
        missing = fenix_remote::MISSING,
        sum = SUM,
    )
}

/// Connects to `target` and starts the helper, installing it first if
/// the machine doesn't have this version's.
pub fn connect(target: &Target, helpers: &Helpers, askpass: Option<&Askpass>) -> Result<Connected, ConnectError> {
    match run(target, helpers, askpass)? {
        Some(session) => Ok(Connected { session, installed: false }),
        None => match run(target, helpers, askpass)? {
            Some(session) => Ok(Connected { session, installed: true }),
            None => Err(ConnectError::Install("it was copied, and still isn't there to run".into())),
        },
    }
}

/// One run of `ssh`: a session, or `None` when the run was spent putting
/// the helper in place.
fn run(target: &Target, helpers: &Helpers, askpass: Option<&Askpass>) -> Result<Option<Session>, ConnectError> {
    let mut child = target.command(&script(), askpass)?.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let (mut stdin, stdout, stderr) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"), child.stderr.take().expect("piped"));
    // What ssh says for itself: the reason, when it fails.
    let said = thread::spawn(move || {
        let mut said = String::new();
        let _ = { stderr }.read_to_string(&mut said);
        said
    });
    let mut reader = BufReader::new(Box::new(stdout) as Box<dyn Read + Send>);
    match preamble(&mut reader) {
        Ok(hello) => match Session::greeted(reader, stdin, hello) {
            Ok(session) => Ok(Some(session.carried_by(child))),
            Err(error) => Err(ConnectError::Start(error)),
        },
        Err(StartError::Missing(system)) => {
            let sent = helpers.for_system(&system).and_then(|helper| Ok(send(&helper, &mut stdin)?));
            // Closed: the far end reads the end of the program, or, when
            // there was none to send, gives up.
            drop(stdin);
            let mut printed = String::new();
            let _ = reader.read_to_string(&mut printed);
            let status = child.wait()?;
            let said = said.join().unwrap_or_default();
            let sent = sent?;
            let arrived = printed.lines().find_map(|line| line.trim().strip_prefix(SUM)).unwrap_or_default().trim();
            if status.success() && arrived.eq_ignore_ascii_case(&sent) {
                Ok(None)
            } else if !arrived.is_empty() && !arrived.eq_ignore_ascii_case(&sent) {
                Err(ConnectError::Install(format!("what arrived isn't what was sent ({arrived}, not {sent})")))
            } else {
                Err(ConnectError::Install(if said.trim().is_empty() { "the remote machine wouldn't take it".into() } else { said }))
            }
        }
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

/// Sends the helper: its SHA-256 on a line, then the program. Gives the
/// SHA-256 back.
fn send(helper: &Path, to: &mut impl Write) -> io::Result<String> {
    let bytes = std::fs::read(helper)?;
    let sum: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    writeln!(to, "{sum}")?;
    to.write_all(&bytes)?;
    to.flush()?;
    Ok(sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_remote_machine_is_given_no_agent_and_no_display() {
        let command = Target::new("devbox").command("true", None).unwrap();
        let args: Vec<String> = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let host = args.iter().position(|arg| arg == "devbox").unwrap();
        for option in ["ForwardAgent=no", "ForwardX11=no"] {
            let at = args.iter().position(|arg| arg == option).unwrap_or_else(|| panic!("{option} in {args:?}"));
            assert!(at < host && args[at - 1] == "-o", "{args:?}");
        }
    }

    #[test]
    fn a_machine_is_matched_to_its_helper() {
        assert_eq!(helper_name("Linux x86_64").as_deref(), Some("fenix-remote-linux-x86_64"));
        assert_eq!(helper_name("Linux aarch64").as_deref(), Some("fenix-remote-linux-aarch64"));
        assert_eq!(helper_name("Linux arm64").as_deref(), Some("fenix-remote-linux-aarch64"));
        for other in ["Darwin arm64", "Linux riscv64", "FreeBSD amd64", "", "Linux"] {
            assert_eq!(helper_name(other), None, "{other}");
        }
    }

    #[test]
    fn a_refusal_is_not_tried_again_and_an_absence_is() {
        assert!(!ConnectError::Ssh("tom@devbox: Permission denied (publickey,password).".into()).worth_retrying());
        assert!(!ConnectError::Ssh("Host key verification failed.".into()).worth_retrying());
        assert!(ConnectError::Ssh("ssh: connect to host devbox port 22: Connection timed out".into()).worth_retrying());
        assert!(ConnectError::Ssh("ssh: Could not resolve hostname devbox: No such host is known.".into()).worth_retrying());
        assert!(!ConnectError::Unsupported("Darwin arm64".into()).worth_retrying());
    }
}
