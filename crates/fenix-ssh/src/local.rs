//! A listener on this machine only, for the two small programs that
//! talk back to Fenix: the askpass hook and the stand-in.
//!
//! A loopback TCP port and a token. Fenix's control pipe can't carry a
//! program's stdin and stdout at once (one synchronous handle, so a
//! waiting read holds up every write), so the real thing wants an
//! overlapped pipe, or a Unix socket, of its own.
//!
//! Any program on this machine can connect to a loopback port, another
//! user's included. So nothing is done for a caller before it has said
//! the token: 256 bits from the system's generator, given to the two
//! programs in their environment (which other users can't read), and
//! compared without telling by its timing how much of a guess was right.
//! A caller gets a few seconds and a bounded line to say it in.

use std::io::{self, BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

/// How long a caller has to say who it is.
const PATIENCE: Duration = Duration::from_secs(10);

/// The longest first line a caller may send: a token and a question, or
/// a token and a command line.
const LONGEST_LINE: u64 = 1024 * 1024;

/// Where a listener is, and what a caller has to say to be heard.
#[derive(Debug, Clone)]
pub struct Door {
    pub addr: SocketAddr,
    pub token: String,
}

/// Listens on a loopback port; `serve` gets each connection on a thread
/// of its own. Runs until the process ends.
pub(crate) fn listen(name: &str, serve: impl Fn(TcpStream, &str) + Send + Sync + 'static) -> io::Result<Door> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let door = Door { addr: listener.local_addr()?, token: token()? };
    let token = door.token.clone();
    let serve = std::sync::Arc::new(serve);
    thread::Builder::new().name(name.into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_nodelay(true);
            let (serve, token) = (serve.clone(), token.clone());
            thread::spawn(move || serve(stream, &token));
        }
    })?;
    Ok(door)
}

pub(crate) fn knock(addr: &str) -> io::Result<TcpStream> {
    let stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

/// A caller's first line: read within `PATIENCE`, and refused past
/// `LONGEST_LINE`, so that a connection that says nothing, or never
/// stops, costs a thread for seconds and a bounded buffer. The reader
/// that comes back has no time limit: what follows may be a program's
/// stdin, which waits as long as its user does.
pub(crate) fn first_line(stream: &TcpStream) -> io::Result<(String, BufReader<TcpStream>)> {
    stream.set_read_timeout(Some(PATIENCE))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = Vec::new();
    (&mut reader).take(LONGEST_LINE).read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no line, or one too long"));
    }
    stream.set_read_timeout(None)?;
    String::from_utf8(line).map(|line| (line, reader)).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not text"))
}

/// Whether `given` is `token`, taking the same time whatever part of it
/// is right.
pub(crate) fn same_token(given: &str, token: &str) -> bool {
    given.len() == token.len() && given.bytes().zip(token.bytes()).fold(0u8, |differ, (a, b)| differ | (a ^ b)) == 0
}

/// 256 bits from the operating system's generator.
fn token() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| io::Error::other(format!("no randomness from the system: {error}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_token_is_long_and_never_the_same() {
        let (a, b) = (token().unwrap(), token().unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(same_token(&a, &a) && !same_token(&a, &b) && !same_token(&a[..63], &a) && !same_token("", &a));
    }

    #[test]
    fn a_caller_that_says_too_much_or_nothing_is_dropped() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let mut polite = knock(&addr).unwrap();
        writeln!(polite, "hello").unwrap();
        let (line, _) = first_line(&listener.accept().unwrap().0).unwrap();
        assert_eq!(line, "hello\n");

        // More than a line's worth, with no end to it.
        let mut endless = knock(&addr).unwrap();
        let accepted = listener.accept().unwrap().0;
        let flooding = thread::spawn(move || {
            let chunk = vec![b'x'; 64 * 1024];
            while endless.write_all(&chunk).is_ok() {}
        });
        assert_eq!(first_line(&accepted).unwrap_err().kind(), io::ErrorKind::InvalidData);
        drop(accepted);
        flooding.join().unwrap();

        // One that closes without a word.
        drop(knock(&addr).unwrap());
        assert!(first_line(&listener.accept().unwrap().0).is_err());
    }
}
