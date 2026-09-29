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
//! reproduced then leaves behind where it happened.
//!
//! Time the machine spent asleep isn't counted: the monitor notices its
//! own second-long wait took far longer, and starts the clock again.

use std::cell::Cell;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Busy this long, and it's reported.
const STUCK_AFTER: Duration = Duration::from_secs(5);
/// The log is started afresh (the old one kept as `.old`) past this.
const MAX_LOG: u64 = 512 * 1024;

struct State {
    /// What the UI thread is in, outermost first.
    stack: Vec<&'static str>,
    /// When the stack last went from empty to busy.
    since: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { stack: Vec::new(), since: None });

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
/// writing to `log`.
pub fn start(log: PathBuf) {
    UI.with(|ui| ui.set(true));
    let _ = std::thread::Builder::new().name("fenix-watchdog".into()).spawn(move || monitor(log));
}

fn monitor(log: PathBuf) {
    let tick = Duration::from_secs(1);
    let mut last = Instant::now();
    // The stall being reported: when it began (as the stack says), what
    // it was in when first reported, and how long it has been seen to
    // last so far.
    let mut reported: Option<(Instant, String, Duration)> = None;
    // Where the clock starts again after the machine slept.
    let mut woke = Instant::now();
    loop {
        std::thread::sleep(tick);
        let now = Instant::now();
        if now.duration_since(last) > tick * 5 {
            woke = now;
        }
        last = now;
        let (since, stack) = {
            let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
            (state.since, state.stack.join(" > "))
        };
        match (since, reported.take()) {
            // The same stall, still going.
            (Some(since), Some((began, what, _))) if since == began => {
                reported = Some((began, what, now.duration_since(since.max(woke))));
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

fn write(log: &PathBuf, line: &str) {
    if std::env::var_os("FENIX_PROFILE").is_some() {
        eprintln!("fenix-watchdog: {line}");
    }
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
}
