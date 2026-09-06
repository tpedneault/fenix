//! One worker owns a task tree and polls both pipes. UI cancellation/drop never
//! joins a worker or waits for a process. Callbacks must return promptly.

use crate::TaskDef;
use process_wrap::std::{ChildWrapper, CommandWrap};
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunId(u64);
impl RunId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}
#[derive(Debug, Clone)]
pub struct OutputLine {
    pub stream: OutputStream,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskOutcome {
    Exited { success: bool, code: Option<i32> },
    Cancelled,
    Failed(String),
}
#[derive(Debug, Clone)]
pub enum TaskEvent {
    Output(Vec<OutputLine>),
    Finished(TaskOutcome),
}

/// Dropping the handle requests cancellation. The worker keeps owning the job
/// and pipes until bounded cleanup completes, even after the UI closes a pane.
pub struct TaskRunner {
    cancel: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}
impl TaskRunner {
    pub fn spawn(
        task: &TaskDef,
        root: &Path,
        send: impl FnMut(TaskEvent) -> bool + Send + 'static,
    ) -> io::Result<Self> {
        let mut command = Command::new(fenix_rpc::resolve_command(&task.command));
        command.args(&task.args).current_dir(root);
        Self::spawn_command(command, send)
    }

    pub fn spawn_command(
        mut command: Command,
        send: impl FnMut(TaskEvent) -> bool + Send + 'static,
    ) -> io::Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut command = CommandWrap::from(command);
        #[cfg(windows)]
        let job = kill_on_close_job(&mut command)?;
        #[cfg(windows)]
        {
            use process_wrap::std::{CreationFlags, JobObject};
            use windows::Win32::System::Threading::CREATE_NO_WINDOW;
            command
                .wrap(CreationFlags(CREATE_NO_WINDOW))
                .wrap(JobObject);
        }
        #[cfg(unix)]
        {
            command.wrap(process_wrap::std::ProcessGroup::leader());
        }
        let mut child = ManagedChild {
            inner: command.spawn()?,
            #[cfg(windows)]
            _job: Some(job),
        };
        let stdout = Pipe::new(
            child
                .inner
                .stdout()
                .take()
                .ok_or_else(|| io::Error::other("missing stdout"))?,
            OutputStream::Stdout,
        )?;
        let stderr = Pipe::new(
            child
                .inner
                .stderr()
                .take()
                .ok_or_else(|| io::Error::other("missing stderr"))?,
            OutputStream::Stderr,
        )?;
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let (stop, done) = (cancel.clone(), finished.clone());
        std::thread::Builder::new()
            .name("fenix-task".into())
            .spawn(move || {
                supervise(child, stdout, stderr, stop, send);
                done.store(true, Ordering::Release);
            })?;
        Ok(Self { cancel, finished })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}
impl Drop for TaskRunner {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct ManagedChild {
    inner: Box<dyn ChildWrapper>,
    #[cfg(windows)]
    _job: Option<Arc<std::os::windows::io::OwnedHandle>>,
}
impl ManagedChild {
    #[cfg(test)]
    fn fixture(child: std::process::Child) -> Self {
        Self {
            inner: Box::new(child),
            #[cfg(windows)]
            _job: None,
        }
    }
}
impl Drop for ManagedChild {
    fn drop(&mut self) {
        // Also covers failures while setting up pipes or starting the worker.
        // start_kill is deliberately used instead of kill (which waits).
        let _ = self.inner.start_kill();
        let _ = self.inner.try_wait();
    }
}

/// process-wrap's std JobObject supervises cancellation but does not set
/// KILL_ON_JOB_CLOSE. An outer ownership job also guarantees cleanup if Fenix
/// exits before its worker runs, or is terminated without running destructors.
/// Assignment runs in post_spawn while JobObject still has the child suspended.
#[cfg(windows)]
fn kill_on_close_job(
    command: &mut CommandWrap,
) -> io::Result<Arc<std::os::windows::io::OwnedHandle>> {
    use process_wrap::std::CommandWrapper;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::*;
    #[derive(Debug)]
    struct AssignOwnership(Arc<OwnedHandle>);
    impl CommandWrapper for AssignOwnership {
        fn post_spawn(
            &mut self,
            _: &mut Command,
            child: &mut std::process::Child,
            _: &CommandWrap,
        ) -> io::Result<()> {
            if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), child.as_raw_handle()) }
                == 0
            {
                let error = io::Error::last_os_error();
                let _ = child.kill();
                return Err(error);
            }
            Ok(())
        }
    }
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let job = Arc::new(unsafe { OwnedHandle::from_raw_handle(raw) });
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            raw,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    command.wrap(AssignOwnership(job.clone()));
    Ok(job)
}

fn supervise<A: ReadyRead, B: ReadyRead>(
    mut child: ManagedChild,
    mut stdout: Pipe<A>,
    mut stderr: Pipe<B>,
    cancel: Arc<AtomicBool>,
    mut send: impl FnMut(TaskEvent) -> bool,
) {
    let mut status: Option<ExitStatus> = None;
    let mut ending: Option<(Instant, TaskOutcome)> = None;
    let mut connected = true;
    loop {
        if status.is_none() {
            // Check the leader, not a blocking wait for all job members.
            match child.inner.try_wait() {
                Ok(value) => status = value,
                Err(err) => {
                    ending.get_or_insert((
                        Instant::now(),
                        TaskOutcome::Failed(format!("waiting for task: {err}")),
                    ));
                    let _ = child.inner.start_kill();
                }
            }
        }
        if ending.is_none() && (cancel.load(Ordering::Acquire) || status.is_some()) {
            let outcome = if cancel.load(Ordering::Acquire) {
                TaskOutcome::Cancelled
            } else {
                let status = status.unwrap();
                TaskOutcome::Exited {
                    success: status.success(),
                    code: status.code(),
                }
            };
            // A task owns its descendants even when the leader exits normally.
            let outcome = match child.inner.start_kill() {
                Ok(()) => outcome,
                #[cfg(unix)]
                Err(err) if err.raw_os_error() == Some(libc::ESRCH) => outcome,
                Err(err) => TaskOutcome::Failed(format!("terminating task tree: {err}")),
            };
            ending = Some((Instant::now(), outcome));
        }
        let mut lines = Vec::new();
        for result in [stdout.poll(&mut lines), stderr.poll(&mut lines)] {
            if let Err(err) = result {
                ending = Some((
                    Instant::now(),
                    TaskOutcome::Failed(format!("reading task output: {err}")),
                ));
                let _ = child.inner.start_kill();
            }
        }
        if connected && !lines.is_empty() && !send(TaskEvent::Output(lines)) {
            connected = false;
            cancel.store(true, Ordering::Release);
        }
        if let Some((started, outcome)) = &ending {
            let drained = status.is_some() && stdout.eof && stderr.eof;
            if drained || started.elapsed() >= DRAIN_TIMEOUT {
                let outcome = if drained {
                    outcome.clone()
                } else {
                    TaskOutcome::Failed(format!(
                        "task cleanup exceeded {} seconds; output pipes closed",
                        DRAIN_TIMEOUT.as_secs()
                    ))
                };
                // All available output precedes this one terminal event.
                if connected {
                    let _ = send(TaskEvent::Finished(outcome));
                }
                return;
            }
        }
        std::thread::sleep(POLL);
    }
}

trait ReadyRead: Read {
    fn configure(&self) -> io::Result<()>;
    fn ready_read(&mut self, bytes: &mut [u8]) -> io::Result<usize>;
}
#[cfg(windows)]
impl<T: Read + std::os::windows::io::AsRawHandle> ReadyRead for T {
    fn configure(&self) -> io::Result<()> {
        Ok(())
    }
    fn ready_read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};
        let mut available = 0;
        // This worker is the pipe's only reader, so the reported bytes cannot
        // be consumed elsewhere between PeekNamedPipe and read.
        let ok = unsafe {
            PeekNamedPipe(
                self.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                Ok(0)
            } else {
                Err(err)
            };
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(available as usize);
        self.read(&mut bytes[..count])
    }
}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd> ReadyRead for T {
    fn configure(&self) -> io::Result<()> {
        let flags = unsafe { libc::fcntl(self.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(self.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn ready_read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.read(bytes)
    }
}

struct Pipe<R> {
    reader: R,
    pending: Vec<u8>,
    stream: OutputStream,
    eof: bool,
}
impl<R: ReadyRead> Pipe<R> {
    fn new(reader: R, stream: OutputStream) -> io::Result<Self> {
        reader.configure()?;
        Ok(Self {
            reader,
            pending: Vec::new(),
            stream,
            eof: false,
        })
    }
    fn emit(&mut self, lines: &mut Vec<OutputLine>) {
        if self.pending.last() == Some(&b'\r') {
            self.pending.pop();
        }
        lines.push(OutputLine {
            stream: self.stream,
            text: String::from_utf8_lossy(&self.pending).into_owned(),
        });
        self.pending.clear();
    }
    fn poll(&mut self, lines: &mut Vec<OutputLine>) -> io::Result<()> {
        if self.eof {
            return Ok(());
        }
        let mut bytes = [0; 8192];
        // Bound each stream's work per tick so a flood cannot starve cancel.
        for _ in 0..8 {
            match self.reader.ready_read(&mut bytes) {
                Ok(0) => {
                    self.eof = true;
                    if !self.pending.is_empty() {
                        self.emit(lines);
                    }
                    break;
                }
                Ok(count) => {
                    for &byte in &bytes[..count] {
                        if byte == b'\n' {
                            self.emit(lines);
                        } else {
                            self.pending.push(byte);
                            // Bound unbroken/binary output rather than accumulating forever.
                            if self.pending.len() == MAX_LINE {
                                self.emit(lines);
                            }
                        }
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    self.eof = true;
                    return Err(err);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc::{self, Receiver};

    fn fixture_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args([
            "--ignored",
            "--exact",
            "supervisor::tests::process_fixture",
            "--nocapture",
        ]);
        command.env("FENIX_PROCESS_FIXTURE", mode);
        command
    }

    // Executed only as an isolated subprocess by the lifecycle tests below.
    #[test]
    #[ignore]
    fn process_fixture() {
        match std::env::var("FENIX_PROCESS_FIXTURE").as_deref() {
            Ok("output") => {
                std::io::stdout()
                    .write_all(b"OUT\r\ninvalid:\xff\ntail")
                    .unwrap();
                std::io::stderr().write_all(b"ERR\n").unwrap();
                std::process::exit(7);
            }
            Ok("sleep") => {
                println!("READY {}", std::process::id());
                std::thread::sleep(Duration::from_secs(30));
            }
            Ok(mode @ ("tree" | "orphan")) => {
                let mut child = fixture_command("sleep")
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap();
                if mode == "orphan" {
                    std::thread::sleep(Duration::from_millis(500));
                    std::process::exit(0);
                }
                let _ = child.wait();
            }
            Ok("flood") => {
                println!("READY {}", std::process::id());
                let mut out = std::io::stdout().lock();
                loop {
                    out.write_all(&[b'x'; 8192]).unwrap();
                }
            }
            Ok("host") => {
                let (_runner, rx) = start("sleep");
                println!("CHILD_PID {}", ready(&rx));
                std::io::stdout().flush().unwrap();
                let mut byte = [0];
                std::io::stdin().read_exact(&mut byte).unwrap();
                // Simulate editor exit without Drop or cancellation running.
                std::process::exit(0);
            }
            _ => panic!("fixture must be launched by a lifecycle test"),
        }
    }

    fn start(mode: &str) -> (TaskRunner, Receiver<TaskEvent>) {
        let (tx, rx) = mpsc::channel();
        let runner =
            TaskRunner::spawn_command(fixture_command(mode), move |e| tx.send(e).is_ok()).unwrap();
        (runner, rx)
    }
    fn finish(rx: &Receiver<TaskEvent>) -> (TaskOutcome, Vec<OutputLine>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut lines = Vec::new();
        loop {
            match rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("task must finish within deadline")
            {
                TaskEvent::Output(batch) => lines.extend(batch),
                TaskEvent::Finished(outcome) => return (outcome, lines),
            }
        }
    }
    fn ready(rx: &Receiver<TaskEvent>) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let event = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            match event {
                TaskEvent::Output(lines) => {
                    for line in lines {
                        if let Some(pid) = line.text.strip_prefix("READY ") {
                            return pid.trim().parse().unwrap();
                        }
                    }
                }
                TaskEvent::Finished(outcome) => panic!("finished before ready: {outcome:?}"),
            }
        }
    }

    #[test]
    fn drains_both_streams_and_unterminated_invalid_utf8_before_exit() {
        let (_runner, rx) = start("output");
        let (outcome, lines) = finish(&rx);
        assert_eq!(
            outcome,
            TaskOutcome::Exited {
                success: false,
                code: Some(7)
            }
        );
        assert!(lines
            .iter()
            .any(|l| l.stream == OutputStream::Stdout && l.text == "OUT"));
        assert!(lines
            .iter()
            .any(|l| l.stream == OutputStream::Stderr && l.text == "ERR"));
        assert!(lines.iter().any(|l| l.text == "tail"));
        assert!(lines.iter().any(|l| l.text == "invalid:\u{fffd}"));
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "one final event, no late output"
        );
    }

    #[test]
    fn cancellation_and_drop_return_immediately() {
        let (runner, rx) = start("sleep");
        ready(&rx);
        let started = Instant::now();
        runner.cancel();
        runner.cancel();
        drop(runner);
        assert!(started.elapsed() < Duration::from_millis(250));
        assert_eq!(finish(&rx).0, TaskOutcome::Cancelled);
    }

    #[test]
    fn dropping_handle_cancels_without_explicit_cancel() {
        let (runner, rx) = start("sleep");
        ready(&rx);
        drop(runner);
        assert_eq!(finish(&rx).0, TaskOutcome::Cancelled);
    }

    #[test]
    fn output_flood_cannot_starve_cancellation_or_grow_a_line_unbounded() {
        let (runner, rx) = start("flood");
        ready(&rx);
        runner.cancel();
        let (outcome, lines) = finish(&rx);
        assert_eq!(outcome, TaskOutcome::Cancelled);
        assert!(lines.iter().all(|l| l.text.len() <= MAX_LINE));
    }

    #[test]
    fn spawn_error_keeps_os_error_detail() {
        let result = TaskRunner::spawn_command(
            Command::new("fenix-definitely-missing-command-012345"),
            |_| true,
        );
        assert!(result.is_err());
        assert!(!result.err().unwrap().to_string().is_empty());
    }

    struct UnavailablePipe {
        fail: bool,
    }
    impl Read for UnavailablePipe {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            if self.fail {
                Err(io::Error::other("injected pipe error"))
            } else {
                Err(io::ErrorKind::WouldBlock.into())
            }
        }
    }
    impl ReadyRead for UnavailablePipe {
        fn configure(&self) -> io::Result<()> {
            Ok(())
        }
        fn ready_read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.read(bytes)
        }
    }

    #[test]
    fn pipes_that_never_close_have_a_bounded_cleanup_deadline() {
        let child = fixture_command("sleep")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut events = Vec::new();
        let start = Instant::now();
        supervise(
            ManagedChild::fixture(child),
            Pipe::new(UnavailablePipe { fail: false }, OutputStream::Stdout).unwrap(),
            Pipe::new(UnavailablePipe { fail: false }, OutputStream::Stderr).unwrap(),
            Arc::new(AtomicBool::new(true)),
            |event| {
                events.push(event);
                true
            },
        );
        assert!(start.elapsed() < DRAIN_TIMEOUT + Duration::from_secs(2));
        assert!(
            matches!(events.last(), Some(TaskEvent::Finished(TaskOutcome::Failed(error))) if error.contains("cleanup exceeded"))
        );
    }

    #[test]
    fn pipe_errors_reach_the_consumer() {
        let child = fixture_command("sleep")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut events = Vec::new();
        supervise(
            ManagedChild::fixture(child),
            Pipe::new(UnavailablePipe { fail: true }, OutputStream::Stdout).unwrap(),
            Pipe::new(UnavailablePipe { fail: true }, OutputStream::Stderr).unwrap(),
            Arc::new(AtomicBool::new(false)),
            |event| {
                events.push(event);
                true
            },
        );
        assert!(
            matches!(events.last(), Some(TaskEvent::Finished(TaskOutcome::Failed(error))) if error.contains("injected pipe error"))
        );
    }

    #[cfg(windows)]
    fn assert_tree_terminated(mode: &str, cancel: bool) {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        };
        let (runner, rx) = start(mode);
        let descendant = ready(&rx);
        // Hold the actual process handle before cancellation, avoiding PID reuse.
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, descendant) };
        assert!(!handle.is_null());
        if cancel {
            runner.cancel();
        }
        let outcome = finish(&rx).0;
        let stopped = unsafe { WaitForSingleObject(handle, 2000) };
        unsafe {
            CloseHandle(handle);
        }
        assert_eq!(
            stopped, WAIT_OBJECT_0,
            "descendant must not outlive its task"
        );
        if cancel {
            assert_eq!(outcome, TaskOutcome::Cancelled);
        } else {
            assert_eq!(
                outcome,
                TaskOutcome::Exited {
                    success: true,
                    code: Some(0)
                }
            );
        }
    }
    #[cfg(windows)]
    #[test]
    fn cancellation_terminates_descendants_holding_inherited_pipes() {
        assert_tree_terminated("tree", true);
    }
    #[cfg(windows)]
    #[test]
    fn leader_exit_terminates_orphaned_descendants() {
        assert_tree_terminated("orphan", false);
    }

    #[cfg(windows)]
    #[test]
    fn host_exit_without_destructors_terminates_its_task() {
        use std::io::{BufRead, BufReader};
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
        };
        // The host is deliberately NOT spawned through TaskRunner: only its
        // own ownership job can terminate the nested task on abrupt exit.
        let mut host = fixture_command("host")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = host.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(pid) = line.strip_prefix("CHILD_PID ") {
                    let _ = tx.send(pid.parse::<u32>().unwrap());
                }
            }
        });
        let pid = match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(pid) => pid,
            Err(error) => {
                let _ = host.kill();
                let _ = host.wait();
                panic!("host did not start task: {error}");
            }
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null());
        host.stdin.take().unwrap().write_all(b"x").unwrap();
        let stopped = unsafe { WaitForSingleObject(handle, 5000) };
        unsafe {
            CloseHandle(handle);
        }
        let _ = host.kill();
        let _ = host.wait();
        assert_eq!(
            stopped, WAIT_OBJECT_0,
            "OS must terminate task when host handles close"
        );
    }
}
