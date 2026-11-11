//! The helper's terminals: a program in a pseudo-terminal of its own,
//! as `ssh -t` would give it, so a shell there prompts, colours and
//! edits its line as it does for anyone sitting at the machine.

use std::io;

use crate::Size;

/// A pty's controlling end, kept to give it a new size.
pub struct Master {
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

/// A program started in a new pty, and the ways in and out of it.
pub struct Started {
    pub child: std::process::Child,
    pub master: Master,
    pub reader: Box<dyn io::Read + Send>,
    pub writer: Box<dyn io::Write + Send>,
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::{Master, Size, Started};

    fn winsize(size: Size) -> libc::winsize {
        libc::winsize { ws_row: size.rows.max(1), ws_col: size.cols.max(1), ws_xpixel: 0, ws_ypixel: 0 }
    }

    fn check(result: libc::c_int) -> io::Result<libc::c_int> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    pub fn start(mut command: Command, size: Size) -> io::Result<Started> {
        let (mut master, mut slave) = (0, 0);
        let size = winsize(size);
        // SAFETY: two out-parameters and a size; no name or termios asked.
        check(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), &size) })?;
        // SAFETY: both were just opened, and are owned here alone.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // The program gets the slave end only; the master isn't left open
        // in it, or the pty would never close.
        // SAFETY: a flag on a descriptor owned here.
        check(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) })?;
        command.stdin(Stdio::from(slave.try_clone()?)).stdout(Stdio::from(slave.try_clone()?)).stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe calls, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                // A session of its own, the pty its terminal: the shell
                // has job control, and the whole session can be ended.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        // Dropping the command closes this process's copies of the slave
        // end: once the program and all it started let go of theirs, a
        // read of the master ends.
        drop(command);
        let reader = File::from(master.try_clone()?);
        let writer = File::from(master.try_clone()?);
        Ok(Started { child, master: Master { fd: master }, reader: Box::new(reader), writer: Box::new(writer) })
    }

    pub fn resize(master: &Master, size: Size) -> io::Result<()> {
        let size = winsize(size);
        // SAFETY: a size given to a pty's master end owned here.
        check(unsafe { libc::ioctl(master.fd.as_raw_fd(), libc::TIOCSWINSZ as _, &size) }).map(drop)
    }
}

/// Starts `command` in a new pty of `size`.
#[cfg(unix)]
pub fn start(command: std::process::Command, size: Size) -> io::Result<Started> {
    unix::start(command, size)
}

#[cfg(not(unix))]
pub fn start(_: std::process::Command, _: Size) -> io::Result<Started> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "the helper has no terminals on this system"))
}

/// Gives the terminal a new size; the program is told (`SIGWINCH`).
#[cfg(unix)]
pub fn resize(master: &Master, size: Size) -> io::Result<()> {
    unix::resize(master, size)
}

#[cfg(not(unix))]
pub fn resize(_: &Master, _: Size) -> io::Result<()> {
    Ok(())
}
