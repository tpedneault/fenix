//! Opt-in latency tracing: launch with FENIX_PROFILE=1 and capture stderr.
use std::time::Instant;

/// A span of work: timed with FENIX_PROFILE set, and -- on the UI thread
/// -- named in the hang log while it runs (`watchdog`).
pub struct Scope(Option<(&'static str, Instant)>, #[allow(dead_code)] Option<crate::watchdog::Busy>);

impl Scope {
    pub fn mark(&self, stage: &'static str) {
        if let Some((name, start)) = self.0 {
            eprintln!("fenix-profile {name}/{stage}: {:.3} ms cumulative", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    pub fn new(name: &'static str) -> Self {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let busy = crate::watchdog::enter(name);
        Self(
            if *ENABLED.get_or_init(|| std::env::var_os("FENIX_PROFILE").is_some()) { Some((name, Instant::now())) } else { None },
            busy,
        )
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some((name, start)) = self.0 {
            eprintln!("fenix-profile {name}: {:.3} ms", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

/// Prints how long after launch `stage` was reached, with FENIX_PROFILE
/// set: the first call starts the clock, so `main` calls it first.
pub fn launch_mark(stage: &'static str) {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    if std::env::var_os("FENIX_PROFILE").is_some() {
        eprintln!("fenix-profile launch/{stage}: {:.1} ms", start.elapsed().as_secs_f64() * 1000.0);
    }
}
