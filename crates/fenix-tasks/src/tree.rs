//! A long-lived child process and everything it starts, as one: what a
//! coding agent is (`npx` starts node, which starts the agent, which
//! starts its own tools). `TaskRunner` owns its child's pipes and polls
//! them itself; a `ProcessTree` hands its pipes to the caller -- an
//! agent's stdio is a protocol, read and written by `fenix-acp` -- and
//! only owns the tree's lifetime: killed whole on `kill` or drop, and on
//! Windows also when Fenix itself goes away without running destructors.

use process_wrap::std::{ChildWrapper, CommandWrap};
use std::io;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};

pub struct ProcessTree {
    child: Box<dyn ChildWrapper>,
    #[cfg(windows)]
    _job: std::sync::Arc<std::os::windows::io::OwnedHandle>,
}

impl ProcessTree {
    /// Spawns `command` -- its stdio as the caller set it -- as the leader
    /// of a process group (Unix) or in a job object that kills the tree
    /// when closed (Windows), without a console window of its own.
    pub fn spawn(command: Command) -> io::Result<Self> {
        let mut command = CommandWrap::from(command);
        #[cfg(windows)]
        let job = crate::supervisor::kill_on_close_job(&mut command)?;
        #[cfg(windows)]
        {
            use process_wrap::std::{CreationFlags, JobObject};
            use windows::Win32::System::Threading::CREATE_NO_WINDOW;
            command.wrap(CreationFlags(CREATE_NO_WINDOW)).wrap(JobObject);
        }
        #[cfg(unix)]
        {
            command.wrap(process_wrap::std::ProcessGroup::leader());
        }
        Ok(Self {
            child: command.spawn()?,
            #[cfg(windows)]
            _job: job,
        })
    }

    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin().take()
    }

    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout().take()
    }

    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr().take()
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Kills the whole tree and reaps the leader. Killing a tree that
    /// already ended is not an error.
    pub fn kill(&mut self) {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.child.kill();
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    #[test]
    fn killing_the_leader_kills_what_it_started() {
        let dir = std::env::temp_dir().join(format!("fenix-tree-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pid_file = dir.join("grandchild.pid");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("sleep 30 & echo $! > {}; wait", pid_file.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut tree = ProcessTree::spawn(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let grandchild = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file).ok().and_then(|s| s.trim().parse::<i32>().ok()) {
                break pid;
            }
            assert!(Instant::now() < deadline, "the grandchild never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        tree.kill();
        let deadline = Instant::now() + Duration::from_secs(5);
        // kill(pid, 0) fails once the grandchild is gone (or a zombie
        // reaped by init).
        while unsafe { libc::kill(grandchild, 0) } == 0 {
            assert!(Instant::now() < deadline, "the grandchild outlived its tree");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
