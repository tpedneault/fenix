//! A remote program that looks like a local one.
//!
//! Much of Fenix runs a program by building a `std::process::Command`
//! and reading its pipes: Git, search, language servers, tasks. For a
//! remote project that `Command` is the stand-in, a small local process
//! that joins the open connection through a [`Bridge`] and passes stdin,
//! stdout, stderr and the exit code through. The caller can't tell.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::Command;
use std::sync::Arc;
use std::thread;

use fenix_remote::{Exec, Exit, Failure, FailureCode, Frame, Kind};
use serde::{Deserialize, Serialize};

use crate::local::{self, Door};
use crate::session::{Error, Output, Session};

/// The stand-in's first argument.
pub const STAND_IN: &str = "--remote-exec";

/// Where the stand-in finds its bridge, and what to say to be heard. In
/// the environment, which other users of the machine can't read, and not
/// in the arguments, which on Linux they can.
const ADDR: &str = "FENIX_REMOTE_EXEC_ADDR";
const TOKEN: &str = "FENIX_REMOTE_EXEC_TOKEN";

/// A variable of this prefix on the stand-in is the remote program's,
/// without the prefix: see [`env`].
const ENV: &str = "FENIX_REMOTE_ENV_";

/// Sets a variable for the remote program a stand-in `command` runs.
/// (`Command::env` would set it for the stand-in, here, where nothing
/// reads it.)
pub fn env(command: &mut Command, key: &str, value: &str) {
    command.env(format!("{ENV}{key}"), value);
}

#[derive(Serialize, Deserialize)]
struct Ask {
    token: String,
    exec: Exec,
}

/// Runs stand-ins' programs over a session.
pub struct Bridge {
    door: Door,
}

impl Bridge {
    /// `session` gives the connection to run a program over, each time
    /// one is asked for: after a reconnect it's another.
    pub fn start(session: impl Fn() -> Result<Arc<Session>, Error> + Send + Sync + 'static) -> io::Result<Bridge> {
        let door = local::listen("fenix-remote-exec", move |stream, token| {
            let _ = serve(&session, stream, token);
        })?;
        Ok(Bridge { door })
    }

    /// A `Command` that runs `program` on the remote machine, in `cwd`
    /// there. Its stdin, stdout, stderr and exit code are the program's.
    pub fn command(&self, program: &str, cwd: Option<&str>) -> io::Result<Command> {
        let mut command = Command::new(std::env::current_exe()?);
        command.arg(STAND_IN).arg(cwd.unwrap_or("")).arg(program).env(ADDR, self.door.addr.to_string()).env(TOKEN, &self.door.token);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        Ok(command)
    }
}

fn serve(session: &impl Fn() -> Result<Arc<Session>, Error>, stream: std::net::TcpStream, token: &str) -> io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let ask: Ask = serde_json::from_str(&line).map_err(io::Error::other)?;
    if ask.token != token {
        return Ok(());
    }
    let refuse = |writer: &mut std::net::TcpStream, message: String| Frame::json(Kind::Err, 0, &Failure { code: FailureCode::Other, message }).write(writer);
    let process = match session().and_then(|session| session.exec(ask.exec)) {
        Ok(process) => process,
        Err(error) => return refuse(&mut writer, error.to_string()),
    };

    let input = process.input();
    thread::spawn(move || {
        while let Ok(Some(frame)) = Frame::read(&mut reader) {
            let sent = match frame.kind {
                Kind::Body => input.write(&frame.payload),
                Kind::BodyEnd => input.close(),
                _ => Ok(()),
            };
            if sent.is_err() {
                return;
            }
        }
        // The stand-in went away, killed by whoever started it: so does
        // the program. A no-op once it has ended.
        let _ = input.kill();
    });

    loop {
        let frame = match process.next() {
            Ok(Output::Stdout(bytes)) => Frame::new(Kind::Stdout, 0, bytes),
            Ok(Output::Stderr(bytes)) => Frame::new(Kind::Stderr, 0, bytes),
            Ok(Output::Exit(code)) => {
                Frame::json(Kind::Exit, 0, &Exit { code }).write(&mut writer)?;
                return writer.flush();
            }
            Err(error) => return refuse(&mut writer, error.to_string()),
        };
        if frame.write(&mut writer).is_err() {
            return process.input().kill().map_err(io::Error::other);
        }
    }
}

/// When this process is a stand-in (`--remote-exec cwd program args…`),
/// runs the program through the bridge and gives the
/// exit code to end with. `None` for any other run.
pub fn stand_in_if_asked() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(STAND_IN) {
        return None;
    }
    let (Ok(addr), Ok(token), Some(cwd), Some(program)) = (std::env::var(ADDR), std::env::var(TOKEN), args.next(), args.next()) else {
        eprintln!("fenix: {STAND_IN} is run by Fenix itself");
        return Some(2);
    };
    let env = std::env::vars().filter_map(|(key, value)| key.strip_prefix(ENV).map(|key| (key.to_string(), value))).collect();
    let exec = Exec { program, args: args.collect(), cwd: Some(cwd).filter(|cwd| !cwd.is_empty()), env, stdin: true };
    Some(match stand_in(&addr, &Ask { token, exec }) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("fenix: {error}");
            127
        }
    })
}

fn stand_in(addr: &str, ask: &Ask) -> io::Result<i32> {
    let mut stream = local::knock(addr)?;
    writeln!(stream, "{}", serde_json::to_string(ask).expect("an ask serializes"))?;

    let mut to_bridge = stream.try_clone()?;
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if Frame::new(Kind::Body, 0, &buffer[..n]).write(&mut to_bridge).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = Frame::new(Kind::BodyEnd, 0, []).write(&mut to_bridge);
    });

    let (mut stdout, mut stderr) = (io::stdout().lock(), io::stderr().lock());
    let mut from_bridge = BufReader::new(stream);
    loop {
        let Some(frame) = Frame::read(&mut from_bridge)? else {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Fenix closed the connection"));
        };
        match frame.kind {
            Kind::Stdout => stdout.write_all(&frame.payload).and_then(|()| stdout.flush())?,
            Kind::Stderr => stderr.write_all(&frame.payload)?,
            Kind::Exit => {
                let exit: Exit = serde_json::from_slice(&frame.payload).map_err(io::Error::other)?;
                // A signal's end, as a shell would report it.
                return Ok(exit.code.unwrap_or(137));
            }
            Kind::Err => {
                let failure: Failure = serde_json::from_slice(&frame.payload).map_err(io::Error::other)?;
                return Err(io::Error::other(failure.message));
            }
            _ => {}
        }
    }
}
