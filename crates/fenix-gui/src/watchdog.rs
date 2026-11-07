//! Hang reports: when the UI thread is stuck, `fenix-hangs.log` (in
//! Fenix's local folder, beside `state`) says for how long and what it was
//! doing.
//!
//! The UI thread keeps a small stack of what it's in the middle of: the
//! event it's handling, and every `profile::Scope` it enters beneath that
//! (the redraw, the disk poll, syncing a language server...). A monitor
//! thread looks at it once a second. When the stack has been busy for
//! longer than `STUCK_AFTER`, it writes a line naming it, and another
//! when it's free again, with the total. A freeze that can't be
//! reproduced then leaves behind where it happened. One that lasts
//! `DUMP_AFTER` leaves a minidump too (Windows): the stack says it's in a
//! redraw, the dump says which call into the driver never came back.
//!
//! Time the machine spent asleep isn't counted: the monitor notices its
//! own second-long wait took far longer, and starts the clock again. That
//! notice is also how the app learns the machine slept (`on_wake`): winit
//! doesn't say.
//!
//! The same log takes everything else that's needed to tell what happened
//! to a window that stopped: the GPU in use, sleeps, surfaces failing, the
//! device lost and made again (`note`), and panics. On Windows nothing
//! else would ever see them, Fenix having no console to print to.

use std::cell::Cell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// Busy this long, and it's reported.
const STUCK_AFTER: Duration = Duration::from_secs(5);
/// Busy this long, and it's a hang worth a dump.
const DUMP_AFTER: Duration = Duration::from_secs(30);
/// The monitor's own wait running this many times over is the machine
/// having slept, not a busy machine.
const SLEPT_AFTER: u32 = 5;
/// The log is started afresh (the old one kept as `.old`) past this.
const MAX_LOG: u64 = 512 * 1024;

struct State {
    /// What the UI thread is in, outermost first.
    stack: Vec<&'static str>,
    /// When the stack last went from empty to busy.
    since: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { stack: Vec::new(), since: None });
/// Where this run's log goes, once `start` has said.
static LOG: OnceLock<PathBuf> = OnceLock::new();
/// Told how long the machine slept, each time it wakes.
static ON_WAKE: Mutex<Option<Box<dyn Fn(Duration) + Send>>> = Mutex::new(None);

thread_local! {
    static UI: Cell<bool> = const { Cell::new(false) };
}

/// The UI thread is in `what` until the guard drops. A no-op on any other
/// thread -- `profile::Scope`s are made on worker threads too.
pub fn enter(what: &'static str) -> Option<Busy> {
    if !UI.with(Cell::get) {
        return None;
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.stack.is_empty() {
        state.since = Some(Instant::now());
    }
    state.stack.push(what);
    Some(Busy)
}

pub struct Busy;

impl Drop for Busy {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.stack.pop();
        if state.stack.is_empty() {
            state.since = None;
        }
    }
}

/// Marks the calling thread as the UI thread and starts the monitor,
/// writing to `log`. Panics from here on are written there too.
pub fn start(log: PathBuf) {
    UI.with(|ui| ui.set(true));
    let _ = LOG.set(log.clone());
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        note(&format!("panic on {}: {info}\n{}", thread.name().unwrap_or("a thread"), std::backtrace::Backtrace::force_capture()));
        previous(info);
    }));
    let _ = std::thread::Builder::new().name("fenix-watchdog".into()).spawn(move || monitor(log));
}

/// `wake` is called, on the monitor's thread, each time the machine has
/// been asleep, with for how long.
pub fn on_wake(wake: impl Fn(Duration) + Send + 'static) {
    *ON_WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(wake));
}

/// A line for the log, and for stderr, should there be anyone reading it.
pub fn note(line: &str) {
    eprintln!("fenix: {line}");
    if let Some(log) = LOG.get() {
        append(log, line);
    }
}

/// Whether a wait of `tick` having taken this long (by the two clocks)
/// means the machine slept, and for how long. By both: which of them
/// counts time asleep depends on the platform.
fn slept_for(by_monotonic: Duration, by_wall: Duration, tick: Duration) -> Option<Duration> {
    let longest = by_monotonic.max(by_wall);
    (longest > tick * SLEPT_AFTER).then_some(longest)
}

fn monitor(log: PathBuf) {
    let tick = Duration::from_secs(1);
    let mut last = Instant::now();
    let mut last_wall = SystemTime::now();
    // The stall being reported: when it began (as the stack says), what
    // it was in when first reported, and how long it has been seen to
    // last so far.
    let mut reported: Option<(Instant, String, Duration)> = None;
    // Where the clock starts again after the machine slept.
    let mut woke = Instant::now();
    // One dump a run: it's the first hang that's worth seeing, and a dump
    // stops every thread while it's taken.
    let mut dumped = false;
    loop {
        std::thread::sleep(tick);
        let now = Instant::now();
        let now_wall = SystemTime::now();
        // The wall clock can go back; that's no sleep.
        let by_wall = now_wall.duration_since(last_wall).unwrap_or_default();
        if let Some(slept) = slept_for(now.duration_since(last), by_wall, tick) {
            woke = now;
            write(&log, &format!("woke: asleep for {}", spoken(slept)));
            if let Some(wake) = ON_WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                wake(slept);
            }
        }
        last = now;
        last_wall = now_wall;
        let (since, stack) = {
            let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
            (state.since, state.stack.join(" > "))
        };
        match (since, reported.take()) {
            // The same stall, still going.
            (Some(since), Some((began, what, _))) if since == began => {
                let busy = now.duration_since(since.max(woke));
                if busy >= DUMP_AFTER && !dumped {
                    dumped = true;
                    let dump = log.with_file_name("fenix-hang.dmp");
                    let said = match write_dump(&dump) {
                        Ok(()) => format!("still stuck after {:.0} s in {what}; every thread's stack is in {}", busy.as_secs_f64(), dump.display()),
                        Err(why) => format!("still stuck after {:.0} s in {what}; no dump ({why})", busy.as_secs_f64()),
                    };
                    write(&log, &said);
                }
                reported = Some((began, what, busy));
            }
            // It ended (or a new one started): say how long it was.
            (_, Some((_, what, lasted))) => {
                write(&log, &format!("free again after {:.0} s (was stuck in {what})", lasted.as_secs_f64().max(STUCK_AFTER.as_secs_f64())));
                // A new stall starting the very tick the last one ended
                // is looked at next tick.
            }
            (Some(since), None) => {
                let busy = now.duration_since(since.max(woke));
                if busy >= STUCK_AFTER {
                    write(&log, &format!("stuck for {:.0} s in {stack}", busy.as_secs_f64()));
                    reported = Some((since, stack, busy));
                }
            }
            (None, None) => {}
        }
    }
}

/// `2 h 14 min`, as a sleep is read.
fn spoken(time: Duration) -> String {
    let minutes = time.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => format!("{} s", time.as_secs()),
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} h {m} min"),
    }
}

fn write(log: &Path, line: &str) {
    if std::env::var_os("FENIX_PROFILE").is_some() {
        eprintln!("fenix-watchdog: {line}");
    }
    append(log, line);
}

fn append(log: &Path, line: &str) {
    if std::fs::metadata(log).is_ok_and(|m| m.len() > MAX_LOG) {
        let _ = std::fs::rename(log, log.with_extension("log.old"));
    }
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "{stamp}  {line}");
    }
}

/// Where hang reports go: `fenix-hangs.log` in Fenix's local folder.
pub fn log_path() -> Option<PathBuf> {
    fenix_storage::paths::Roots::current().map(|roots| roots.local.join("fenix-hangs.log"))
}

/// A minidump of this process at `path`, replacing the last: the stacks of
/// all its threads, and which modules they're in. Small (a few megabytes),
/// and with no memory contents beyond the stacks.
#[cfg(windows)]
fn write_dump(path: &Path) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Diagnostics::Debug::{MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId};

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    // SAFETY: the handles are this process's and the file's, alive for the
    // call; the three optional parameters are documented as nullable.
    let wrote = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file.as_raw_handle() as _,
            MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if wrote == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(not(windows))]
fn write_dump(_: &Path) -> Result<(), String> {
    Err("only on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_ui_thread_is_tracked_and_the_stack_unwinds() {
        // Another thread's scope never touches it.
        std::thread::spawn(|| assert!(enter("worker").is_none())).join().unwrap();
        UI.with(|ui| ui.set(true));
        {
            let _outer = enter("user event");
            let _inner = enter("disk probe");
            let state = STATE.lock().unwrap();
            assert_eq!(state.stack.join(" > "), "user event > disk probe");
            assert!(state.since.is_some());
        }
        let state = STATE.lock().unwrap();
        assert!(state.stack.is_empty() && state.since.is_none());
    }

    /// A loaded machine runs a one-second wait a bit long; that's no sleep.
    /// On a platform whose monotonic clock stops asleep, the wall clock
    /// tells; on one where it's the wall clock that was stepped back, the
    /// monotonic one does.
    #[test]
    fn a_sleep_is_a_wait_far_longer_than_it_asked_for_by_either_clock() {
        let tick = Duration::from_secs(1);
        let (short, hours) = (Duration::from_millis(1200), Duration::from_secs(3 * 3600));
        assert_eq!(slept_for(short, short, tick), None);
        assert_eq!(slept_for(short, hours, tick), Some(hours));
        assert_eq!(slept_for(hours, Duration::ZERO, tick), Some(hours));
    }

    #[test]
    fn a_sleep_reads_as_a_person_would_say_it() {
        assert_eq!(spoken(Duration::from_secs(42)), "42 s");
        assert_eq!(spoken(Duration::from_secs(600)), "10 min");
        assert_eq!(spoken(Duration::from_secs(2 * 3600 + 14 * 60 + 9)), "2 h 14 min");
    }

    /// The dump is what says which call never came back, so it has to
    /// actually be written, from a thread other than the one it's for.
    #[cfg(windows)]
    #[test]
    fn a_dump_can_be_taken_of_this_process() {
        let dir = std::env::temp_dir().join(format!("fenix-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fenix-hang.dmp");
        let writer = path.clone();
        std::thread::spawn(move || write_dump(&writer)).join().unwrap().unwrap();
        let size = std::fs::metadata(&path).unwrap().len();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(size > 1024, "{size} bytes");
        assert!(size < 256 * 1024 * 1024, "{size} bytes: too much to ask anyone to keep");
    }
}
